use claude_client::{Client, ContentBlock, Message, MessagesRequest, Role, StopReason};
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
