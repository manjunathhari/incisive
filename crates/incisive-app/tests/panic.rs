//! Panic hygiene: a panic never reaches logs or the browser with its payload
//! (a str-slicing panic message embeds up to 256 bytes of the string, which
//! could be question text).
//!
//! Own test binary: the panic hook and the capture subscriber are process-wide.

use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use http_body_util::BodyExt;
use incisive_app::server::{PANIC_BODY, catch_panic_layer, install_panic_hook};
use tower::ServiceExt;

const SECRET: &str = "zqxsecret Mrs Lakshmi 9845012345";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn panics() -> &'static str {
    let s = String::from(SECRET);
    // A str-slicing panic: the message quotes the string.
    let cut = &s[0..s.len() + 1];
    Box::leak(cut.to_string().into_boxed_str())
}

#[tokio::test]
async fn panics_log_location_only_and_return_a_fixed_500() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .expect("first and only global subscriber in this binary");
    install_panic_hook();

    // A handler panic becomes a fixed 500 with no payload.
    let app = Router::new()
        .route("/boom", get(panics))
        .layer(catch_panic_layer());
    let res = app
        .oneshot(Request::get("/boom").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body, PANIC_BODY);
    assert_eq!(
        PANIC_BODY,
        "Incisive hit an unexpected error. Please try again."
    );

    // A plain panic outside any request is logged the same way.
    let r = std::panic::catch_unwind(|| panic!("plain {SECRET}"));
    assert!(r.is_err());

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("ERROR"), "{logs}");
    assert_eq!(
        logs.matches("tests/panic.rs:").count(),
        2,
        "each panic logs its file:line\n{logs}"
    );
    assert!(
        !logs.contains("zqxsecret"),
        "payload reached the log:\n{logs}"
    );
    assert!(!logs.contains("Lakshmi"), "{logs}");
}
