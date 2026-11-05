//! `stream_message` against a small local server that sends its pieces with
//! real gaps (no network, no real API).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use claude_client::{
    BreakKind, ClaudeError, Client, ContentBlock, Message, MessagesRequest, RetryPolicy, Role,
    StopReason,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// What one connection is sent.
#[derive(Clone)]
enum Step {
    /// Bytes for the body, then flushed.
    Send(String),
    Wait(u64),
}

/// One response: status and steps; when the steps end, the body ends properly
/// (chunked terminator) if `clean`, otherwise the connection is cut.
#[derive(Clone)]
struct Script {
    status: u16,
    steps: Vec<Step>,
    clean: bool,
}

fn ok(steps: Vec<Step>) -> Script {
    Script {
        status: 200,
        steps,
        clean: true,
    }
}

fn cut(steps: Vec<Step>) -> Script {
    Script {
        status: 200,
        steps,
        clean: false,
    }
}

fn sse(v: Value) -> Step {
    Step::Send(format!(
        "event: {}\ndata: {v}\n\n",
        v["type"].as_str().unwrap_or("x")
    ))
}

struct Server {
    base: String,
    bodies: Arc<Mutex<Vec<Value>>>,
}

/// Serve `scripts` in order, one per connection.
async fn serve(scripts: Vec<Script>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = bodies.clone();
    tokio::spawn(async move {
        for script in scripts {
            let (mut sock, _) = listener.accept().await.unwrap();
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut piece = [0u8; 8192];
                loop {
                    let n = sock.read(&mut piece).await.unwrap();
                    buf.extend_from_slice(&piece[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len: usize = text[..end]
                            .lines()
                            .find_map(|l| {
                                let l = l.to_ascii_lowercase();
                                l.strip_prefix("content-length: ")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            seen.lock().unwrap().push(
                                serde_json::from_slice(&buf[end + 4..end + 4 + len]).unwrap(),
                            );
                            break;
                        }
                    }
                }
                let head = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                    script.status
                );
                sock.write_all(head.as_bytes()).await.unwrap();
                for step in &script.steps {
                    match step {
                        Step::Send(s) => {
                            let frame = format!("{:x}\r\n{s}\r\n", s.len());
                            if sock.write_all(frame.as_bytes()).await.is_err() {
                                return;
                            }
                            let _ = sock.flush().await;
                        }
                        Step::Wait(ms) => tokio::time::sleep(Duration::from_millis(*ms)).await,
                    }
                }
                if script.clean {
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    Server { base, bodies }
}

fn client(s: &Server) -> Client {
    Client::with_base_url(SecretString::from("k".to_string()), s.base.parse().unwrap())
        .with_retry_policy(RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
        })
}

fn request() -> MessagesRequest {
    MessagesRequest {
        model: "m".into(),
        max_tokens: 100,
        system: Some("s".into()),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: "u".into() }],
        }],
        tools: vec![],
        tool_choice: None,
        output_config: None,
        cache_breakpoints: vec![],
    }
}

fn start() -> Value {
    json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant",
        "content":[],"model":"m","stop_reason":null,
        "usage":{"input_tokens":11,"output_tokens":1,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}}})
}

fn text_delta(index: u32, t: &str) -> Value {
    json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":t}})
}

/// Thinking, then `{"a":"héllo"}` in pieces, with every kind of event.
fn full_reply() -> Vec<Step> {
    vec![
        sse(start()),
        sse(json!({"type":"ping"})),
        sse(json!({"type":"something_new","x":1})),
        sse(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        ),
        sse(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hidden thought"}}),
        ),
        sse(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
        ),
        sse(json!({"type":"content_block_stop","index":0})),
        sse(
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        ),
        sse(text_delta(1, "{\"a\":")),
        sse(json!({"type":"content_block_delta","index":1,"delta":{"type":"new_delta","x":1}})),
        sse(text_delta(1, "\"héllo\"}")),
        sse(json!({"type":"content_block_stop","index":1})),
        sse(
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":42}}),
        ),
        sse(json!({"type":"message_stop"})),
    ]
}

fn collector() -> (Arc<Mutex<Vec<String>>>, impl Fn(&str) + Send + Sync) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s = seen.clone();
    (seen, move |t: &str| s.lock().unwrap().push(t.to_string()))
}

#[tokio::test]
async fn rebuilds_the_same_reply_as_a_plain_call() {
    let plain = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1",
            "content": [
                {"type":"thinking","thinking":"hidden thought","signature":"sig"},
                {"type":"text","text":"{\"a\":\"héllo\"}"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens":11,"output_tokens":42,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}
        })))
        .mount(&plain)
        .await;
    let want = Client::with_base_url(
        SecretString::from("k".to_string()),
        plain.uri().parse().unwrap(),
    )
    .create_message(&request())
    .await
    .unwrap();

    let s = serve(vec![ok(full_reply())]).await;
    let (seen, on_text) = collector();
    let got = client(&s)
        .stream_message(&request(), &on_text, None)
        .await
        .unwrap();
    assert_eq!(got.id, want.id);
    assert_eq!(got.content, want.content);
    assert_eq!(got.stop_reason, want.stop_reason);
    assert_eq!(got.usage, want.usage);
    // Text so far, never the thinking.
    assert_eq!(*seen.lock().unwrap(), ["{\"a\":", "{\"a\":\"héllo\"}"]);
    // The same request, with `stream` added.
    let sent = s.bodies.lock().unwrap()[0].clone();
    let mut plain_body = request().body();
    plain_body["stream"] = json!(true);
    assert_eq!(sent, plain_body);
}

#[tokio::test]
async fn a_tool_use_reply_is_rebuilt() {
    let s = serve(vec![ok(vec![
        sse(start()),
        sse(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"f","input":{}}})),
        sse(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":"}})),
        sse(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"1}"}})),
        sse(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}})),
        sse(json!({"type":"message_stop"})),
    ])])
    .await;
    let got = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap();
    assert_eq!(
        got.content,
        [ContentBlock::ToolUse {
            id: "t1".into(),
            name: "f".into(),
            input: json!({"q":1})
        }]
    );
    assert_eq!(got.stop_reason, Some(StopReason::ToolUse));
}

#[tokio::test]
async fn max_tokens_arrives_through_message_delta() {
    let s = serve(vec![ok(vec![
        sse(start()),
        sse(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        sse(text_delta(0, "{\"a\":\"cut")),
        sse(json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":100}})),
        sse(json!({"type":"message_stop"})),
    ])])
    .await;
    let got = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap();
    assert_eq!(got.stop_reason, Some(StopReason::MaxTokens));
    assert_eq!(got.usage.output_tokens, 100);
}

#[tokio::test]
async fn an_error_event_after_the_start_is_an_interruption_with_counts() {
    let s = serve(vec![ok(vec![
        sse(start()),
        sse(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        sse(text_delta(0, "12345")),
        sse(json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}})),
    ])])
    .await;
    let e = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap_err();
    let ClaudeError::StreamInterrupted(b) = e else {
        panic!("{e:?}")
    };
    assert_eq!(b.kind, BreakKind::ApiError);
    assert!(b.started);
    assert_eq!(b.output_bytes, 5);
    assert_eq!(b.usage.unwrap().input_tokens, 11);
    // Never retried here: one connection only.
    assert_eq!(s.bodies.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_error_event_first_takes_the_retry_policy() {
    let s = serve(vec![
        ok(vec![sse(
            json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
        )]),
        ok(full_reply()),
    ])
    .await;
    let got = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap();
    assert_eq!(got.usage.output_tokens, 42);
    assert_eq!(s.bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_status_error_takes_the_retry_policy() {
    let s = serve(vec![
        Script {
            status: 529,
            steps: vec![],
            clean: true,
        },
        ok(full_reply()),
    ])
    .await;
    let got = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap();
    assert_eq!(got.stop_reason, Some(StopReason::EndTurn));
    assert_eq!(s.bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_status_error_with_one_attempt_is_final() {
    let s = serve(vec![Script {
        status: 529,
        steps: vec![],
        clean: true,
    }])
    .await;
    let e = client(&s)
        .with_retry_policy(RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        })
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap_err();
    assert!(matches!(e, ClaudeError::Overloaded), "{e:?}");
}

#[tokio::test]
async fn a_cut_connection_is_an_interruption_and_is_not_retried() {
    let s = serve(vec![cut(vec![
        sse(start()),
        sse(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        sse(text_delta(0, "abc")),
    ])])
    .await;
    let e = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap_err();
    let ClaudeError::StreamInterrupted(b) = e else {
        panic!("{e:?}")
    };
    assert_eq!(b.kind, BreakKind::Transport);
    assert_eq!(b.output_bytes, 3);
    assert_eq!(s.bodies.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_body_ending_with_no_message_stop_is_an_interruption() {
    let s = serve(vec![ok(vec![sse(start())])]).await;
    let e = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap_err();
    let ClaudeError::StreamInterrupted(b) = e else {
        panic!("{e:?}")
    };
    assert_eq!(b.kind, BreakKind::EndedEarly);
}

#[tokio::test]
async fn an_event_that_cannot_be_read_is_an_interruption() {
    let s = serve(vec![ok(vec![
        sse(start()),
        Step::Send("data: {not json\n\n".into()),
        Step::Wait(500),
    ])])
    .await;
    let e = client(&s)
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap_err();
    let ClaudeError::StreamInterrupted(b) = e else {
        panic!("{e:?}")
    };
    assert_eq!(b.kind, BreakKind::Malformed);
}

#[tokio::test]
async fn idle_with_no_event_breaks_after_the_limit() {
    let s = serve(vec![ok(vec![sse(start()), Step::Wait(5_000)])]).await;
    let t = Instant::now();
    let e = client(&s)
        .with_stream_idle_timeout(Duration::from_millis(150))
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap_err();
    let ClaudeError::StreamInterrupted(b) = e else {
        panic!("{e:?}")
    };
    assert_eq!(b.kind, BreakKind::Idle);
    assert!(b.started);
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[test]
fn the_idle_limit_is_sixty_seconds() {
    assert_eq!(claude_client::STREAM_IDLE_TIMEOUT, Duration::from_secs(60));
}

#[tokio::test]
async fn a_long_silence_with_pings_does_not_trip_the_idle_limit() {
    let mut steps = vec![sse(start())];
    for _ in 0..8 {
        steps.push(Step::Wait(60));
        steps.push(sse(json!({"type":"ping"})));
    }
    steps.extend(full_reply().into_iter().skip(1));
    let s = serve(vec![ok(steps)]).await;
    let got = client(&s)
        .with_stream_idle_timeout(Duration::from_millis(200))
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap();
    assert_eq!(got.stop_reason, Some(StopReason::EndTurn));
}

/// A reply past the per-attempt limit of plain calls is not cut.
#[tokio::test]
async fn a_writer_past_the_request_timeout_is_not_cut() {
    let mut steps = vec![sse(start())];
    for _ in 0..6 {
        steps.push(Step::Wait(100));
        steps.push(sse(json!({"type":"ping"})));
    }
    steps.extend(full_reply().into_iter().skip(1));
    let s = serve(vec![ok(steps)]).await;
    let got = client(&s)
        .with_request_timeout(Duration::from_millis(200))
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap();
    assert_eq!(got.usage.output_tokens, 42);
}

#[tokio::test]
async fn the_deadline_ends_a_stream_that_keeps_pinging() {
    let mut steps = vec![sse(start())];
    for _ in 0..200 {
        steps.push(Step::Wait(20));
        steps.push(sse(json!({"type":"ping"})));
    }
    let s = serve(vec![ok(steps)]).await;
    let t = Instant::now();
    let e = client(&s)
        .stream_message(
            &request(),
            &|_| {},
            Some(Instant::now() + Duration::from_millis(200)),
        )
        .await
        .unwrap_err();
    let ClaudeError::StreamInterrupted(b) = e else {
        panic!("{e:?}")
    };
    assert_eq!(b.kind, BreakKind::Deadline);
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn silence_before_the_first_event_takes_the_retry_policy() {
    let s = serve(vec![ok(vec![Step::Wait(2_000)]), ok(full_reply())]).await;
    let got = client(&s)
        .with_stream_idle_timeout(Duration::from_millis(150))
        .stream_message(&request(), &|_| {}, None)
        .await
        .unwrap();
    assert_eq!(got.usage.output_tokens, 42);
    assert_eq!(s.bodies.lock().unwrap().len(), 2);
}

/// `stream` is only ever added by `stream_message`.
#[tokio::test]
async fn a_plain_call_never_asks_for_a_stream() {
    let plain = MockServer::start().await;
    Mock::given(body_partial_json(json!({"model": "m"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "m", "content": [], "stop_reason": "end_turn",
            "usage": {"input_tokens":1,"output_tokens":1}
        })))
        .mount(&plain)
        .await;
    Client::with_base_url(
        SecretString::from("k".to_string()),
        plain.uri().parse().unwrap(),
    )
    .create_message(&request())
    .await
    .unwrap();
    let got = plain.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&got[0].body).unwrap();
    assert!(body.get("stream").is_none());
}
