//! Who is asking: Google sign-in via Identity-Aware Proxy (IAP), and the users table.
//!
//! In prod the only identity is the signed `x-goog-iap-jwt-assertion` header,
//! verified (ES256, issuer, audience, expiry) against Google's published keys.
//! Unverified identity headers are never trusted. `x-dev-user-email` is honoured
//! only when `APP_ENV=dev`.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use axum::{
    Json,
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, postgres::PgRow};
use tokio::sync::Mutex;
use url::Url;

use crate::config::AppEnv;

pub const IAP_JWT_HEADER: &str = "x-goog-iap-jwt-assertion";
pub const DEV_EMAIL_HEADER: &str = "x-dev-user-email";
pub const IAP_ISSUER: &str = "https://cloud.google.com/iap";
pub const IAP_JWK_URL: &str = "https://www.gstatic.com/iap/verify/public_key-jwk";
const LEEWAY_SECS: u64 = 30;
/// Once keys are loaded, refetch on an unknown `kid` at most this often.
const KEY_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(300);
/// With no keys at all (cold start, failed first fetch), retry this soon.
const KEY_EMPTY_RETRY_INTERVAL: Duration = Duration::from_secs(10);
const NOT_SET_UP_MESSAGE: &str = "Your Google account is allowed in, but you haven't been set up in Incisive yet. Ask the owner to add you.";

// ---- roles and users ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Surgeon,
    Fellow,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Surgeon => "surgeon",
            Role::Fellow => "fellow",
        }
    }

    fn parse(s: &str) -> Option<Role> {
        match s {
            "owner" => Some(Role::Owner),
            "surgeon" => Some(Role::Surgeon),
            "fellow" => Some(Role::Fellow),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub role: Role,
    pub daily_cap_paise: i64,
    pub monthly_cap_paise: Option<i64>,
    pub active: bool,
}

fn user_from_row(row: &PgRow) -> sqlx::Result<User> {
    let role: String = row.try_get("role")?;
    Ok(User {
        id: row.try_get("id")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        role: Role::parse(&role).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "role".into(),
            source: "unknown role".into(),
        })?,
        daily_cap_paise: row.try_get("daily_cap_paise")?,
        monthly_cap_paise: row.try_get("monthly_cap_paise")?,
        active: row.try_get("active")?,
    })
}

const USER_COLUMNS: &str =
    "id, email, display_name, role, daily_cap_paise, monthly_cap_paise, active";

pub async fn find_user(pool: &PgPool, email: &str) -> sqlx::Result<Option<User>> {
    let row = sqlx::query(&format!(
        "SELECT {USER_COLUMNS} FROM incisive.users WHERE email = $1"
    ))
    .bind(email.trim().to_lowercase())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(user_from_row).transpose()
}

/// Make sure the owner exists, is the owner, and is active. Safe to call on every
/// start. Caps are set only on first creation so later edits are not overwritten.
pub async fn ensure_owner(pool: &PgPool, email: &str, display_name: &str) -> sqlx::Result<User> {
    let (daily, monthly) = crate::spend::default_caps(Role::Owner);
    let row = sqlx::query(&format!(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise) \
         VALUES ($1, $2, 'owner', $3, $4) \
         ON CONFLICT (email) DO UPDATE SET role = 'owner', active = TRUE \
         RETURNING {USER_COLUMNS}"
    ))
    .bind(email.trim().to_lowercase())
    .bind(display_name)
    .bind(daily)
    .bind(monthly)
    .fetch_one(pool)
    .await?;
    user_from_row(&row)
}

/// Retry `ensure_owner` with exponential backoff (`base`, doubling, capped at `cap`)
/// until it succeeds. Logs failures without any secrets.
pub async fn ensure_owner_until_done(
    pool: PgPool,
    email: String,
    display_name: String,
    base: Duration,
    cap: Duration,
) {
    let mut delay = base;
    let mut attempt: u32 = 1;
    loop {
        match ensure_owner(&pool, &email, &display_name).await {
            Ok(_) => {
                tracing::info!(attempt, "owner account ready");
                return;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "owner setup failed; will retry");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(cap);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

// ---- IAP JWT verification ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// No credential was presented.
    Missing,
    /// A credential was presented and rejected; the reason is a fixed string.
    Invalid(&'static str),
    /// Google's signing keys could not be loaded.
    KeysUnavailable,
}

pub enum JwkSource {
    Remote(Url),
    /// Fixed key set, for tests.
    Static(JwkSet),
}

struct KeyCache {
    set: Option<JwkSet>,
    last_fetch: Option<Instant>,
}

pub struct IapVerifier {
    audience: String,
    source: JwkSource,
    cache: Mutex<KeyCache>,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct Claims {
    email: String,
}

impl IapVerifier {
    pub fn new(audience: String, keys: JwkSource) -> Self {
        let cache = match &keys {
            JwkSource::Static(set) => KeyCache {
                set: Some(set.clone()),
                last_fetch: Some(Instant::now()),
            },
            JwkSource::Remote(_) => KeyCache {
                set: None,
                last_fetch: None,
            },
        };
        Self {
            audience,
            source: keys,
            cache: Mutex::new(cache),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("http client"),
        }
    }

    pub async fn verify(&self, jwt: &str) -> Result<VerifiedIdentity, IdentityError> {
        let header = decode_header(jwt).map_err(|_| IdentityError::Invalid("malformed token"))?;
        if header.alg != Algorithm::ES256 {
            return Err(IdentityError::Invalid("unexpected algorithm"));
        }
        let kid = header.kid.ok_or(IdentityError::Invalid("missing key id"))?;
        let jwk = self.key_for(&kid).await?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| IdentityError::Invalid("bad key"))?;

        let mut validation = Validation::new(Algorithm::ES256);
        validation.algorithms = vec![Algorithm::ES256];
        validation.set_issuer(&[IAP_ISSUER]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.validate_exp = true;
        validation.leeway = LEEWAY_SECS;

        let data = decode::<Claims>(jwt, &key, &validation)
            .map_err(|_| IdentityError::Invalid("token failed verification"))?;
        let email = data.claims.email.trim().to_lowercase();
        if email.is_empty() {
            return Err(IdentityError::Invalid("no email in token"));
        }
        Ok(VerifiedIdentity { email })
    }

    /// Find the key with this `kid`. For a remote source, an unknown `kid` triggers
    /// a refetch at most once per five minutes.
    async fn key_for(&self, kid: &str) -> Result<Jwk, IdentityError> {
        let mut cache = self.cache.lock().await;
        if let Some(jwk) = cache.set.as_ref().and_then(|s| s.find(kid)) {
            return Ok(jwk.clone());
        }
        let JwkSource::Remote(url) = &self.source else {
            return Err(IdentityError::Invalid("unknown key id"));
        };
        let min_interval = if cache.set.is_some() {
            KEY_REFRESH_MIN_INTERVAL
        } else {
            KEY_EMPTY_RETRY_INTERVAL
        };
        let due = cache.last_fetch.is_none_or(|t| t.elapsed() >= min_interval);
        if !due {
            return Err(if cache.set.is_some() {
                IdentityError::Invalid("unknown key id")
            } else {
                IdentityError::KeysUnavailable
            });
        }
        cache.last_fetch = Some(Instant::now());
        match self.fetch(url).await {
            Ok(set) => cache.set = Some(set),
            Err(()) => {
                tracing::warn!("could not fetch IAP signing keys");
                if cache.set.is_none() {
                    return Err(IdentityError::KeysUnavailable);
                }
            }
        }
        cache
            .set
            .as_ref()
            .and_then(|s| s.find(kid))
            .cloned()
            .ok_or(IdentityError::Invalid("unknown key id"))
    }

    async fn fetch(&self, url: &Url) -> Result<JwkSet, ()> {
        let resp = self.http.get(url.clone()).send().await.map_err(|_| ())?;
        if !resp.status().is_success() {
            return Err(());
        }
        resp.json::<JwkSet>().await.map_err(|_| ())
    }
}

// ---- axum extractor ----

#[derive(Clone)]
pub struct IdentityState {
    pub verifier: Arc<IapVerifier>,
    pub pool: PgPool,
    pub app_env: AppEnv,
}

impl IdentityState {
    pub fn new(verifier: IapVerifier, pool: PgPool, app_env: AppEnv) -> Self {
        Self {
            verifier: Arc::new(verifier),
            pool,
            app_env,
        }
    }
}

/// The signed-in, set-up, active user making this request.
pub struct CurrentUser(pub User);

#[derive(Debug)]
pub enum IdentityRejection {
    NotSignedIn,
    KeysUnavailable,
    NotSetUp,
    Internal,
}

impl IdentityRejection {
    /// Fixed kind for the audit log and the JSON `error` field.
    pub fn kind(&self) -> &'static str {
        match self {
            IdentityRejection::NotSignedIn => "not_signed_in",
            IdentityRejection::KeysUnavailable => "sign_in_check_unavailable",
            IdentityRejection::NotSetUp => "not_set_up",
            IdentityRejection::Internal => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            IdentityRejection::NotSignedIn => StatusCode::UNAUTHORIZED,
            IdentityRejection::KeysUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            IdentityRejection::NotSetUp => StatusCode::FORBIDDEN,
            IdentityRejection::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Plain advice for the page.
    pub fn message(&self) -> &'static str {
        match self {
            IdentityRejection::NotSignedIn => {
                "You're not signed in to Incisive. Reload the page and sign in with your Google account."
            }
            IdentityRejection::KeysUnavailable => {
                "Sign-in could not be checked just now. Please try again in a moment."
            }
            IdentityRejection::NotSetUp => NOT_SET_UP_MESSAGE,
            IdentityRejection::Internal => {
                "Incisive couldn't check who you are just now. Please try again in a moment."
            }
        }
    }
}

impl IntoResponse for IdentityRejection {
    fn into_response(self) -> Response {
        match self {
            IdentityRejection::NotSignedIn => (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "not_signed_in"})),
            ),
            IdentityRejection::KeysUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "sign_in_check_unavailable",
                    "message": "Sign-in could not be checked just now. Please try again in a moment."
                })),
            ),
            IdentityRejection::NotSetUp => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "not_set_up", "message": NOT_SET_UP_MESSAGE})),
            ),
            IdentityRejection::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal"})),
            ),
        }
        .into_response()
    }
}

impl<S> FromRequestParts<S> for CurrentUser
where
    IdentityState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = IdentityRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let st = IdentityState::from_ref(state);
        let email = match st.app_env {
            AppEnv::Dev => parts
                .headers
                .get(DEV_EMAIL_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().to_lowercase())
                .filter(|v| !v.is_empty())
                .ok_or(IdentityRejection::NotSignedIn)?,
            AppEnv::Prod => {
                let Some(token) = parts
                    .headers
                    .get(IAP_JWT_HEADER)
                    .and_then(|v| v.to_str().ok())
                else {
                    tracing::info!(reason = "no IAP token", "identity rejected");
                    return Err(IdentityRejection::NotSignedIn);
                };
                match st.verifier.verify(token).await {
                    Ok(id) => id.email,
                    Err(e) => {
                        let reason = match e {
                            IdentityError::Missing => "no IAP token",
                            IdentityError::Invalid(r) => r,
                            IdentityError::KeysUnavailable => "signing keys unavailable",
                        };
                        tracing::info!(reason, "identity rejected");
                        return Err(if e == IdentityError::KeysUnavailable {
                            IdentityRejection::KeysUnavailable
                        } else {
                            IdentityRejection::NotSignedIn
                        });
                    }
                }
            }
        };
        match find_user(&st.pool, &email).await {
            Ok(Some(u)) if u.active => Ok(CurrentUser(u)),
            Ok(_) => {
                tracing::info!(reason = "not set up or inactive", "identity rejected");
                Err(IdentityRejection::NotSetUp)
            }
            Err(_) => {
                tracing::warn!("user lookup failed");
                Err(IdentityRejection::Internal)
            }
        }
    }
}
