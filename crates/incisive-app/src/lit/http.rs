//! The one outbound HTTP client for literature sources.
//!
//! The allowlist is a security control (spec §5), enforced here in code:
//!
//! * https only, default port only, no userinfo, no IP literals;
//! * the host must equal (ASCII case-insensitively, after URL parsing) one of
//!   [`BASE_HOSTS`], or the URL must be one an [`OaPolicy`] registered with
//!   [`OaPolicy::allow_once`] (an open-access full-text link taken from a
//!   Europe PMC or Unpaywall response), which can be fetched exactly once;
//! * redirects are followed by hand (at most [`MAX_REDIRECTS`]); every hop
//!   must independently pass the base allowlist. The one-time grant covers only
//!   the originally registered URL, never a redirect target.
//!
//! Refusals are logged at warn level with the host only (never path or query).

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::LOCATION;
use serde::de::DeserializeOwned;
use url::{Host, Url};

/// Hosts that may always be fetched. `api.anthropic.com` is listed for
/// completeness; the Claude client does not use this client.
pub const BASE_HOSTS: [&str; 4] = [
    "eutils.ncbi.nlm.nih.gov",
    "www.ebi.ac.uk",
    "api.unpaywall.org",
    "api.anthropic.com",
];

pub const MAX_REDIRECTS: usize = 3;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Retries after the first attempt, on 5xx or timeout.
pub const MAX_RETRIES: u32 = 2;
/// Cap for `get_text` / `get_json` bodies (metadata responses, not full texts).
pub const DEFAULT_MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LitError {
    /// The URL (or a redirect hop) is not allowed. Carries the host only.
    Disallowed(String),
    TooLarge,
    Http(u16),
    Timeout,
    Parse,
    /// Connection-level failure (DNS, TLS, reset) that is not a timeout.
    Network,
}

impl fmt::Display for LitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LitError::Disallowed(h) => write!(f, "host not allowed: {h}"),
            LitError::TooLarge => write!(f, "response too large"),
            LitError::Http(s) => write!(f, "http status {s}"),
            LitError::Timeout => write!(f, "request timed out"),
            LitError::Parse => write!(f, "could not parse response"),
            LitError::Network => write!(f, "network error"),
        }
    }
}

impl std::error::Error for LitError {}

/// `Incisive/<version> (+https://github.com/manjunathhari/incisive)`
pub fn default_user_agent() -> String {
    format!(
        "Incisive/{} (+https://github.com/manjunathhari/incisive)",
        env!("CARGO_PKG_VERSION")
    )
}

/// One-time grants for open-access full-text URLs. Cheap to clone; clones share
/// state, and registration/consumption is atomic under a mutex, so parallel
/// fetches can never use one grant twice.
#[derive(Clone, Default, Debug)]
pub struct OaPolicy {
    urls: Arc<Mutex<HashSet<String>>>,
}

impl OaPolicy {
    /// Registers one exact URL for a single fetch. Anything that is not a
    /// well-formed https URL is ignored (it would be refused at fetch anyway).
    pub fn allow_once(&self, url: &str) {
        if let Some(u) = parse_https_candidate(url) {
            self.urls.lock().unwrap().insert(u.to_string());
        }
    }

    fn take(&self, url: &Url) -> bool {
        self.urls.lock().unwrap().remove(url.as_str())
    }
}

/// Parses and applies the host-independent rules (https, default port, no
/// userinfo, domain host). Returns `None` when any of them fails.
fn parse_https_candidate(raw: &str) -> Option<Url> {
    let u = Url::parse(raw).ok()?;
    let ok = u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && u.port().is_none() // the url crate drops an explicit :443
        && matches!(u.host(), Some(Host::Domain(d)) if !d.is_empty());
    ok.then_some(u)
}

fn host_for_log(raw: &str) -> String {
    Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "invalid-url".to_string())
}

fn is_base_host(u: &Url) -> bool {
    match u.host() {
        Some(Host::Domain(d)) => BASE_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(d)),
        _ => false,
    }
}

fn refuse(raw_or_host: &str) -> LitError {
    let host = host_for_log(raw_or_host);
    tracing::warn!(host = %host, "lit: request refused by allowlist");
    LitError::Disallowed(host)
}

#[derive(Clone)]
pub struct AllowlistedClient {
    http: reqwest::Client,
    /// Kept so the test-only builders can rebuild `http`.
    #[cfg(any(test, feature = "test-util"))]
    user_agent: String,
    oa: OaPolicy,
    #[cfg(any(test, feature = "test-util"))]
    timeout: Duration,
    backoff: Duration,
    /// Test only: every allowlisted request is sent here instead.
    #[cfg(any(test, feature = "test-util"))]
    base_override: Option<Url>,
}

impl AllowlistedClient {
    pub fn new(user_agent: impl Into<String>, oa: OaPolicy) -> Self {
        let user_agent = user_agent.into();
        Self {
            http: build(&user_agent, REQUEST_TIMEOUT, false),
            #[cfg(any(test, feature = "test-util"))]
            user_agent,
            oa,
            #[cfg(any(test, feature = "test-util"))]
            timeout: REQUEST_TIMEOUT,
            backoff: Duration::from_millis(500),
            #[cfg(any(test, feature = "test-util"))]
            base_override: None,
        }
    }

    /// Base backoff between retries; attempt n waits `n * backoff`.
    pub fn with_retry_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    /// Test only. Sends every request that passes the allowlist to `base`
    /// (a mock server URI) instead of the real host. The allowlist still sees
    /// the original URL, so its behaviour is the production behaviour.
    #[cfg(any(test, feature = "test-util"))]
    pub fn with_base_override(mut self, base: impl AsRef<str>) -> Self {
        self.base_override = Some(Url::parse(base.as_ref()).expect("valid override URL"));
        self.http = build(&self.user_agent, self.timeout, true);
        self
    }

    /// Test only.
    #[cfg(any(test, feature = "test-util"))]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.http = build(&self.user_agent, timeout, self.base_override.is_some());
        self
    }

    pub async fn get_text(&self, url: &str) -> Result<String, LitError> {
        let bytes = self.get_bytes_capped(url, DEFAULT_MAX_BODY).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, LitError> {
        let bytes = self.get_bytes_capped(url, DEFAULT_MAX_BODY).await?;
        serde_json::from_slice(&bytes).map_err(|_| LitError::Parse)
    }

    /// Streams the body and aborts as soon as more than `max` bytes arrive.
    pub async fn get_bytes_capped(&self, url: &str, max: usize) -> Result<Vec<u8>, LitError> {
        // First hop: base allowlist, or consume the one-time OA grant.
        let mut current = Url::parse(url).map_err(|_| refuse(url))?;
        // The grant is consumed here, once; retries of this hop reuse `current`.
        if !self.hop_allowed(&current)
            && !(parse_https_candidate(current.as_str()).is_some() && self.oa.take(&current))
        {
            return Err(refuse(url));
        }

        let mut redirects = 0;
        loop {
            match self.fetch_hop(&current, max).await? {
                Hop::Body(b) => return Ok(b),
                Hop::Redirect(location) => {
                    if redirects >= MAX_REDIRECTS {
                        return Err(LitError::Http(302));
                    }
                    redirects += 1;
                    let next = current.join(&location).map_err(|_| refuse(&location))?;
                    // The one-time OA grant never extends to a redirect target.
                    if !self.hop_allowed(&next) {
                        return Err(refuse(next.as_str()));
                    }
                    current = next;
                }
            }
        }
    }

    /// Base-allowlist check (everything except the one-time OA grant).
    fn hop_allowed(&self, u: &Url) -> bool {
        parse_https_candidate(u.as_str()).is_some() && is_base_host(u)
    }

    #[cfg(any(test, feature = "test-util"))]
    fn wire_url(&self, logical: &Url) -> Url {
        match &self.base_override {
            Some(base) => {
                let mut u = base.clone();
                u.set_path(logical.path());
                u.set_query(logical.query());
                u
            }
            None => logical.clone(),
        }
    }

    #[cfg(not(any(test, feature = "test-util")))]
    fn wire_url(&self, logical: &Url) -> Url {
        logical.clone()
    }

    /// One allowed hop, with retries on 5xx and timeout.
    async fn fetch_hop(&self, logical: &Url, max: usize) -> Result<Hop, LitError> {
        let wire = self.wire_url(logical);
        let mut attempt = 0u32;
        loop {
            let result = self.attempt(&wire, max).await;
            let retryable = matches!(
                result,
                Err(LitError::Timeout) | Err(LitError::Http(500..=599))
            );
            if retryable && attempt < MAX_RETRIES {
                attempt += 1;
                tokio::time::sleep(self.backoff * attempt).await;
                continue;
            }
            return result;
        }
    }

    async fn attempt(&self, wire: &Url, max: usize) -> Result<Hop, LitError> {
        let mut resp = self.http.get(wire.clone()).send().await.map_err(map_err)?;
        let status = resp.status();
        if status.is_redirection() {
            return match resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .filter(|_| matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308))
            {
                Some(l) => Ok(Hop::Redirect(l.to_string())),
                None => Err(LitError::Http(status.as_u16())),
            };
        }
        if !status.is_success() {
            return Err(LitError::Http(status.as_u16()));
        }
        if resp.content_length().is_some_and(|n| n > max as u64) {
            return Err(LitError::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(map_err)? {
            if body.len() + chunk.len() > max {
                return Err(LitError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Hop::Body(body))
    }
}

enum Hop {
    Body(Vec<u8>),
    Redirect(String),
}

fn map_err(e: reqwest::Error) -> LitError {
    if e.is_timeout() {
        LitError::Timeout
    } else {
        LitError::Network
    }
}

fn build(user_agent: &str, timeout: Duration, no_proxy: bool) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    if no_proxy {
        // Test override talks to loopback; never route that through a proxy.
        b = b.no_proxy();
    }
    b.build().expect("reqwest client builds")
}
