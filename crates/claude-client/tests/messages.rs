use std::time::Duration;

use claude_client::{
    ClaudeError, Client, ContentBlock, Effort, Message, MessagesRequest, OutputConfig,
    OutputFormat, RetryPolicy, Role, StopReason, ToolChoice, ToolDef,
};
use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request() -> MessagesRequest {
    MessagesRequest {
        model: "test-model".into(),
        max_tokens: 64,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "Hi".into() }],
        }],
        tools: vec![],
        tool_choice: None,
        output_config: None,
    }
}

fn client(server: &MockServer) -> Client {
    Client::with_base_url(
        SecretString::from("sk-test".to_string()),
        server.uri().parse().unwrap(),
    )
}

async fn serve(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn sends_required_headers_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(
            json!({"model": "test-model", "max_tokens": 64}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1",
            "content": [{"type": "text", "text": "Hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 3, "output_tokens": 5}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let resp = client(&server).create_message(&request()).await.unwrap();
    assert_eq!(
        resp.content,
        vec![ContentBlock::Text {
            text: "Hello".into()
        }]
    );
    assert_eq!(resp.usage.output_tokens, 5);
}

#[tokio::test]
async fn parses_tool_use_and_stop_reason() {
    let server = MockServer::start().await;
    serve(
        &server,
        json!({
            "id": "msg_2",
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "pubmed_search",
                "input": {"query": "ICG leak"}
            }],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        }),
    )
    .await;

    let resp = client(&server).create_message(&request()).await.unwrap();
    assert_eq!(
        resp.content,
        vec![ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "pubmed_search".into(),
            input: json!({"query": "ICG leak"}),
        }]
    );
    assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
}

#[tokio::test]
async fn unknown_block_type_is_tolerated() {
    let server = MockServer::start().await;
    serve(
        &server,
        json!({
            "id": "msg_3",
            "content": [
                {"type": "web_search_tool_result", "content": []},
                {"type": "thinking", "thinking": "..."},
                {"type": "text", "text": "ok"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }),
    )
    .await;

    let resp = client(&server).create_message(&request()).await.unwrap();
    assert_eq!(
        resp.content,
        vec![
            ContentBlock::Unknown,
            // Thinking is a known block now (Phase 3); a missing signature
            // reads as empty rather than failing the reply.
            ContentBlock::Thinking {
                thinking: "...".into(),
                signature: String::new()
            },
            ContentBlock::Text { text: "ok".into() }
        ]
    );
}

fn fast_client(server: &MockServer) -> Client {
    client(server).with_retry_policy(RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(50),
    })
}

fn ok_body() -> serde_json::Value {
    json!({
        "id": "msg_ok",
        "content": [{"type": "text", "text": "ready"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 1}
    })
}

#[tokio::test]
async fn bad_request_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "type": "error",
            "error": {"type": "invalid_request_error", "message": "max_tokens: required"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let err = fast_client(&server)
        .create_message(&request())
        .await
        .unwrap_err();
    match err {
        ClaudeError::Api {
            status,
            error_type,
            message,
        } => {
            assert_eq!(status, 400);
            assert_eq!(error_type, "invalid_request_error");
            assert_eq!(message, "max_tokens: required");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn rate_limit_honours_retry_after_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .expect(1)
        .mount(&server)
        .await;

    let resp = fast_client(&server)
        .create_message(&request())
        .await
        .unwrap();
    assert_eq!(resp.usage.output_tokens, 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn overloaded_gives_up_after_three_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(529).set_body_json(json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        })))
        .expect(3)
        .mount(&server)
        .await;

    let err = fast_client(&server)
        .create_message(&request())
        .await
        .unwrap_err();
    assert!(matches!(err, ClaudeError::Overloaded), "got {err:?}");
}

#[tokio::test]
async fn persistent_429_returns_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "type": "error",
            "error": {"type": "rate_limit_error", "message": "slow down"}
        })))
        .expect(3)
        .mount(&server)
        .await;

    let err = fast_client(&server)
        .create_message(&request())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ClaudeError::RateLimited { .. }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn html_error_page_is_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
        .expect(3)
        .mount(&server)
        .await;

    let err = fast_client(&server)
        .create_message(&request())
        .await
        .unwrap_err();
    match err {
        ClaudeError::Decode {
            status,
            body_snippet,
        } => {
            assert_eq!(status, 502);
            assert!(body_snippet.contains("Bad Gateway"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn debug_does_not_leak_key() {
    let c = Client::new(SecretString::from("sk-live-SECRET".to_string()));
    let shown = format!("{c:?}");
    assert!(!shown.contains("sk-live-SECRET"), "leaked: {shown}");
    assert!(shown.contains("[redacted]"));
}

#[test]
fn tool_choice_is_omitted_when_unset() {
    let body = serde_json::to_value(request()).unwrap();
    let o = body.as_object().unwrap();
    assert!(!o.contains_key("tool_choice"), "{body}");
    assert!(!o.contains_key("tools"), "{body}");
}

#[test]
fn forced_tool_choice_serialises_as_the_api_expects() {
    let mut req = request();
    req.tools = vec![ToolDef {
        name: "submit".into(),
        description: "Submit the answer.".into(),
        input_schema: json!({"type": "object", "properties": {}}),
    }];
    req.tool_choice = Some(ToolChoice::Tool {
        name: "submit".into(),
    });
    let body = serde_json::to_value(&req).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({"type": "tool", "name": "submit"})
    );
    assert_eq!(body["tools"][0]["name"], "submit");
    assert_eq!(
        serde_json::to_value(ToolChoice::Auto).unwrap(),
        json!({"type": "auto"})
    );
    assert_eq!(
        serde_json::to_value(ToolChoice::Any).unwrap(),
        json!({"type": "any"})
    );
}

#[tokio::test]
async fn forced_tool_choice_is_sent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(
            json!({"tool_choice": {"type": "tool", "name": "submit"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_t",
            "content": [{"type": "tool_use", "id": "tu_1", "name": "submit", "input": {"a": 1}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 3, "output_tokens": 5}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let mut req = request();
    req.tools = vec![ToolDef {
        name: "submit".into(),
        description: "d".into(),
        input_schema: json!({"type": "object"}),
    }];
    req.tool_choice = Some(ToolChoice::Tool {
        name: "submit".into(),
    });
    let resp = client(&server).create_message(&req).await.unwrap();
    assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
    assert!(
        matches!(&resp.content[0], ContentBlock::ToolUse { input, .. } if input == &json!({"a": 1}))
    );
}

#[test]
fn output_config_is_omitted_when_unset_and_serialises_json_schema() {
    let body = serde_json::to_value(request()).unwrap();
    assert!(
        !body.as_object().unwrap().contains_key("output_config"),
        "{body}"
    );
    let mut req = request();
    req.output_config = Some(OutputConfig {
        format: Some(OutputFormat::JsonSchema {
            schema: json!({"type": "object", "additionalProperties": false}),
        }),
        effort: None,
    });
    let body = serde_json::to_value(&req).unwrap();
    assert_eq!(
        body["output_config"],
        json!({"format": {"type": "json_schema", "schema": {"type": "object", "additionalProperties": false}}})
    );
    // The deprecated parameter is never sent.
    assert!(!body.as_object().unwrap().contains_key("output_format"));
    assert!(!body.as_object().unwrap().contains_key("tool_choice"));
}

#[tokio::test]
async fn refusal_stop_reason_is_parsed_and_unknown_is_other() {
    let server = MockServer::start().await;
    serve(
        &server,
        json!({
            "id": "msg_r",
            "content": [{"type": "text", "text": "I can't help with that."}],
            "stop_reason": "refusal",
            "usage": {"input_tokens": 3, "output_tokens": 5}
        }),
    )
    .await;
    let resp = client(&server).create_message(&request()).await.unwrap();
    assert_eq!(resp.stop_reason, Some(StopReason::Refusal));
    let other: StopReason = serde_json::from_value(json!("pause_turn")).unwrap();
    assert_eq!(other, StopReason::Other);
}

#[test]
fn output_config_effort_serialises_beside_format_and_is_omitted_when_unset() {
    let mut req = request();
    req.output_config = Some(OutputConfig {
        format: Some(OutputFormat::JsonSchema {
            schema: json!({"type": "object"}),
        }),
        effort: Some(Effort::Low),
    });
    let body = serde_json::to_value(&req).unwrap();
    assert_eq!(
        body["output_config"],
        json!({"format": {"type": "json_schema", "schema": {"type": "object"}}, "effort": "low"})
    );
    for (e, want) in [
        (Effort::Low, "low"),
        (Effort::Medium, "medium"),
        (Effort::High, "high"),
        (Effort::Xhigh, "xhigh"),
        (Effort::Max, "max"),
    ] {
        assert_eq!(serde_json::to_value(e).unwrap(), json!(want));
    }
    req.output_config.as_mut().unwrap().effort = None;
    let body = serde_json::to_value(&req).unwrap();
    assert!(
        !body["output_config"]
            .as_object()
            .unwrap()
            .contains_key("effort"),
        "{body}"
    );
}

// ---------- Phase 3 T1: multi-turn tool use ----------

/// A tool turn: `tools`, no `tool_choice` (auto, the API default) and
/// `output_config` with effort only (no format).
#[test]
fn tool_turn_sends_tools_without_tool_choice_and_effort_only_output_config() {
    let mut req = request();
    req.tools = vec![ToolDef {
        name: "get_abstracts".into(),
        description: "d".into(),
        input_schema: json!({"type": "object", "additionalProperties": false}),
    }];
    req.output_config = Some(OutputConfig {
        format: None,
        effort: Some(Effort::High),
    });
    let body = serde_json::to_value(&req).unwrap();
    let o = body.as_object().unwrap();
    assert!(!o.contains_key("tool_choice"), "{body}");
    assert_eq!(body["output_config"], json!({"effort": "high"}));
    assert_eq!(body["tools"][0]["name"], "get_abstracts");
    // A structured call still sends its format.
    req.output_config = Some(OutputConfig::format(json!({"type": "object"})));
    let body = serde_json::to_value(&req).unwrap();
    assert_eq!(
        body["output_config"],
        json!({"format": {"type": "json_schema", "schema": {"type": "object"}}})
    );
}

/// The assistant turn is sent back as received: thinking blocks keep their
/// signature (the API requires them beside `tool_use` on the next turn),
/// redacted thinking keeps its data, and tool results carry `is_error`.
#[test]
fn thinking_tool_use_and_tool_result_round_trip() {
    let resp: claude_client::MessagesResponse = serde_json::from_value(json!({
        "id": "msg_1",
        "content": [
            {"type": "thinking", "thinking": "let me search", "signature": "sig=="},
            {"type": "redacted_thinking", "data": "opaque"},
            {"type": "text", "text": "Searching."},
            {"type": "tool_use", "id": "toolu_1", "name": "search_pubmed", "input": {"query": "x", "max_results": 5}},
            {"type": "server_tool_use", "id": "srv_1"}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "output_tokens": 20}
    }))
    .unwrap();
    assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
    assert!(matches!(&resp.content[4], ContentBlock::Unknown));
    let back = serde_json::to_value(&resp.content[..4]).unwrap();
    assert_eq!(
        back,
        json!([
            {"type": "thinking", "thinking": "let me search", "signature": "sig=="},
            {"type": "redacted_thinking", "data": "opaque"},
            {"type": "text", "text": "Searching."},
            {"type": "tool_use", "id": "toolu_1", "name": "search_pubmed", "input": {"query": "x", "max_results": 5}}
        ])
    );
    let result = ContentBlock::ToolResult {
        tool_use_id: "toolu_1".into(),
        content: "bad input".into(),
        is_error: true,
    };
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({"type": "tool_result", "tool_use_id": "toolu_1", "content": "bad input", "is_error": true})
    );
}
