use std::time::Duration;

use claude_client::{
    ClaudeError, Client, ContentBlock, Message, MessagesRequest, RetryPolicy, Role, StopReason,
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
