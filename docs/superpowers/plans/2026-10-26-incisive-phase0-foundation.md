# Incisive Phase 0 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A deployed, empty-but-real Incisive: a Rust full-stack app on Google Cloud Run (Mumbai) that serves a placeholder page, reports health, connects to Supabase (Mumbai), and contains a tested Claude Messages API client, all built, checked and deployed automatically from GitHub.

**Architecture:** One Cargo workspace with two crates: `claude-client` (a small library wrapping the Claude Messages API over HTTPS/JSON, with typed errors and retries) and `incisive-app` (Leptos full-stack on axum, serving pages and JSON endpoints). Postgres access via `sqlx` against Supabase. A multi-stage Docker image is deployed to Cloud Run by GitHub Actions using Workload Identity Federation (no stored cloud keys); secrets come from Google Secret Manager.

**Tech Stack:** Rust stable (edition 2024), axum 0.8, Leptos (current release via the `cargo-leptos` `start-axum` template), tokio, reqwest (rustls), serde/serde_json, secrecy, thiserror, tracing + tracing-subscriber (JSON), sqlx (postgres, runtime-tokio, tls-rustls), wiremock (tests), GitHub Actions, Docker, Google Cloud Run, Artifact Registry, Secret Manager, Supabase Postgres.

**Spec:** `specs/surgeon-evidence-sandbox-design.md` (Project doc "Incisive: Design Spec (v1)"), sections 3, 6 and 8 (Phase 0).

## Global Constraints

- Product name in UI copy: **Incisive**; tagline: **"Cut to the evidence."**
- Hosting region: Cloud Run **asia-south1 (Mumbai)**; Supabase **ap-south-1 (Mumbai)**; Artifact Registry **asia-south1**.
- Cloud Run service name: **`incisive-app`**. Request timeout **3600 s**; min instances **0**; max instances **3**; request-based billing (CPU only during requests).
- The Claude API key exists only on the server: never in the browser, never in logs, never in `Debug` output.
- Claude API: `POST https://api.anthropic.com/v1/messages`, headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`.
- Claude model ID is configuration (`CLAUDE_MODEL`), never hard-coded.
- Every CI build runs: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo audit`.
- No identifiable patient data anywhere in Phase 0 (no data model for it yet).
- Budget alert on the Google Cloud billing account at **₹500/month**.

## Review Focus

1. **A required environment variable is missing or blank** (e.g. Secret Manager not wired): the app must exit at startup with a message naming the variable, not panic later inside a request. Test: Task 1.
2. **Supabase unreachable at boot** (free project paused, wrong URL): the app must still start, `/healthz` returns 200, `/readyz` returns 503 with `{"db":"down"}`, so Cloud Run does not crash-loop. Test: Task 4.
3. **Claude API rate-limits or is overloaded** (429 with `retry-after`, or 529): the client retries with backoff at most 3 attempts total, honours `retry-after`, then returns a typed error; never loops forever. Test: Task 3.
4. **Claude API returns something unexpected**: an HTML error page from a proxy, or a response containing a content-block type this client doesn't know yet. HTML gives `ClaudeError::Decode` carrying the HTTP status; an unknown block type decodes as `ContentBlock::Unknown` and does not fail the whole response. Tests: Tasks 2 and 3.
5. **API key leakage**: formatting `Client` or `Config` with `{:?}` must not reveal the key. Test: Tasks 1 and 3.

---

## Before you start (owner, one-time, about 30 minutes)

These need your accounts; an implementer cannot do them.

1. **GitHub:** create a private repository `incisive`.
2. **Google Cloud:** create project `incisive-prod`; attach a billing account (free-trial is fine); enable Cloud Run, Artifact Registry, Secret Manager, IAM Credentials APIs.
3. **Supabase:** create project `incisive` in region **South Asia (Mumbai)**; copy the **session-mode pooler** connection string (port 5432).
4. **Anthropic:** in claude.ai Settings › Billing, link a Claude Console organisation to claim the Max plan's monthly API credits; in the Console, create an API key named `incisive-prod`.
5. Hand the implementer: GitHub repo URL, GCP project ID, and confirmation that the two secrets are ready to be stored (Task 6 stores them; never paste them into chat or code).

---

## File structure

```
incisive/
├── Cargo.toml                      # workspace: members = crates/*
├── rust-toolchain.toml             # channel = "stable"
├── .github/workflows/ci.yml        # fmt, clippy, test, audit (Task 1)
├── .github/workflows/deploy.yml    # build → push → deploy (Task 6)
├── Dockerfile, .dockerignore       # Task 6
├── infra/README.md                 # one-time GCP/Supabase commands (Task 6)
├── migrations/0001_init.sql        # Task 5
└── crates/
    ├── claude-client/
    │   ├── src/lib.rs              # pub use; Client
    │   ├── src/types.rs            # request/response types (Task 2)
    │   ├── src/error.rs            # ClaudeError (Task 3)
    │   ├── src/retry.rs            # backoff policy (Task 3)
    │   ├── examples/ping.rs        # live smoke test (Task 3)
    │   └── tests/messages.rs       # wiremock tests (Tasks 2–3)
    └── incisive-app/
        ├── src/main.rs             # boot: config, tracing, db, serve
        ├── src/config.rs           # Config::from_env (Task 1)
        ├── src/health.rs           # /healthz, /readyz, Readiness trait (Task 4)
        ├── src/db.rs               # PgReadiness, connect, migrate (Task 5)
        ├── src/app.rs              # Leptos root component + router (Task 4)
        └── tests/health.rs         # router tests (Task 4)
```

---

### Task 1: Workspace, configuration and CI

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`, `crates/incisive-app/Cargo.toml`, `crates/incisive-app/src/config.rs`, `crates/incisive-app/src/main.rs` (stub `fn main() {}` until Task 4)
- Test: inline `#[cfg(test)] mod tests` in `config.rs`

**Interfaces:**
- Produces:
  - `pub struct Config { pub anthropic_api_key: SecretString, pub claude_model: String, pub database_url: SecretString, pub app_env: AppEnv, pub port: u16 }`
  - `pub enum AppEnv { Dev, Prod }`
  - `pub fn Config::from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError>` (testable core)
  - `pub fn Config::from_env() -> Result<Config, ConfigError>` (calls `from_lookup` with `std::env::var(..).ok()`)
  - `pub enum ConfigError { Missing(&'static str), Invalid { var: &'static str, reason: String } }` implementing `Display` as `"missing required environment variable ANTHROPIC_API_KEY"` / `"invalid value for PORT: <reason>"`
- Env vars: `ANTHROPIC_API_KEY` (required), `CLAUDE_MODEL` (required), `DATABASE_URL` (required), `APP_ENV` (`dev`|`prod`, default `dev`), `PORT` (default `8080`, as Cloud Run sets it).

- [ ] **Step 1: Write the failing tests** in `config.rs`:
  - `missing_api_key_is_named`: lookup without `ANTHROPIC_API_KEY` → `Err(ConfigError::Missing("ANTHROPIC_API_KEY"))`, and `err.to_string()` contains `"ANTHROPIC_API_KEY"`.
  - `blank_value_counts_as_missing`: `ANTHROPIC_API_KEY = "   "` → `Missing("ANTHROPIC_API_KEY")`.
  - `port_defaults_to_8080_and_env_to_dev`: all required set, no `PORT`/`APP_ENV` → `port == 8080`, `app_env == AppEnv::Dev`.
  - `bad_port_is_invalid`: `PORT = "abc"` → `Err(ConfigError::Invalid { var: "PORT", .. })`.
  - `debug_redacts_secrets`: `format!("{:?}", config)` does not contain the test key `"sk-test-123"` nor the database password.
- [ ] **Step 2: Run** `cargo test -p incisive-app config` → FAIL (items undefined).
- [ ] **Step 3: Implement** `Config`, `AppEnv`, `ConfigError`, `from_lookup`, `from_env` in `config.rs`. Trim values; empty after trim is `Missing`. Use `secrecy::SecretString` for both secrets (its `Debug` is redacted).
- [ ] **Step 4: Run** `cargo test -p incisive-app config` → PASS (5 tests).
- [ ] **Step 5: Add CI** `.github/workflows/ci.yml` on `push` and `pull_request`: checkout; `dtolnay/rust-toolchain@stable` with `rustfmt, clippy`; `Swatinem/rust-cache`; then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo install cargo-audit --locked && cargo audit`.
- [ ] **Step 6: Verify locally** `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` → all succeed.
- [ ] **Step 7: Commit** `git commit -m "chore: workspace, config loading, CI"`; push; confirm the CI run on GitHub is green.

---

### Task 2: Claude client: types and successful call

**Files:**
- Create: `crates/claude-client/Cargo.toml`, `src/lib.rs`, `src/types.rs`, `tests/messages.rs`

**Interfaces:**
- Produces (in `types.rs`, re-exported from `lib.rs`):
  - `pub struct MessagesRequest { pub model: String, pub max_tokens: u32, #[serde(skip_serializing_if = "Option::is_none")] pub system: Option<String>, pub messages: Vec<Message>, #[serde(skip_serializing_if = "Vec::is_empty")] pub tools: Vec<ToolDef> }`
  - `pub struct Message { pub role: Role, pub content: Vec<ContentBlock> }`; `pub enum Role { User, Assistant }` serialised lowercase
  - `pub enum ContentBlock` tagged `#[serde(tag = "type", rename_all = "snake_case")]`: `Text { text: String }`, `ToolUse { id: String, name: String, input: serde_json::Value }`, `ToolResult { tool_use_id: String, content: String, #[serde(default)] is_error: bool }`, `#[serde(other)] Unknown`
  - `pub struct ToolDef { pub name: String, pub description: String, pub input_schema: serde_json::Value }`
  - `pub struct MessagesResponse { pub id: String, pub content: Vec<ContentBlock>, pub stop_reason: Option<StopReason>, pub usage: Usage }`
  - `pub enum StopReason` snake_case: `EndTurn, MaxTokens, StopSequence, ToolUse, #[serde(other)] Other`
  - `pub struct Usage { pub input_tokens: u32, pub output_tokens: u32 }`
  - `pub struct Client` with `pub fn new(api_key: SecretString) -> Client` (base URL `https://api.anthropic.com`), `pub fn with_base_url(api_key: SecretString, base_url: reqwest::Url) -> Client`, `pub async fn create_message(&self, req: &MessagesRequest) -> Result<MessagesResponse, ClaudeError>`
  - `ClaudeError` is defined minimally here (`Transport(reqwest::Error)`, `Decode { status: u16, body_snippet: String }`) and completed in Task 3.

- [ ] **Step 1: Write the failing tests** in `tests/messages.rs` using `wiremock::MockServer`:
  - `sends_required_headers_and_body`: mock `POST /v1/messages` matching headers `x-api-key: sk-test`, `anthropic-version: 2023-06-01`, `content-type: application/json` and a body JSON containing `"model":"test-model"` and `"max_tokens":64`; respond 200 with a text block. Assert `resp.content == vec![ContentBlock::Text { text: "Hello".into() }]` and `resp.usage.output_tokens == 5`.
  - `parses_tool_use_and_stop_reason`: response with a `tool_use` block (`name: "pubmed_search"`, `input: {"query":"ICG leak"}`) and `"stop_reason":"tool_use"` → matching `ContentBlock::ToolUse` and `Some(StopReason::ToolUse)`.
  - `unknown_block_type_is_tolerated`: response content `[{"type":"thinking","thinking":"..."},{"type":"text","text":"ok"}]` → `[ContentBlock::Unknown, ContentBlock::Text{ text: "ok" }]`.
- [ ] **Step 2: Run** `cargo test -p claude-client` → FAIL (types undefined).
- [ ] **Step 3: Implement** the types and `create_message` (single attempt, no retries yet). `reqwest::Client` built with rustls and a 120 s timeout. On 2xx read body as bytes and `serde_json::from_slice`; on decode failure return `ClaudeError::Decode` with status and the first 200 chars of body.
- [ ] **Step 4: Run** `cargo test -p claude-client` → PASS (3 tests).
- [ ] **Step 5: Commit** `git commit -m "feat(claude-client): typed Messages API call"`.

---

### Task 3: Claude client: errors, retries, secrecy, live smoke test

**Files:**
- Create: `crates/claude-client/src/error.rs`, `src/retry.rs`, `examples/ping.rs`
- Modify: `src/lib.rs` (`create_message` uses retry policy), `tests/messages.rs`

**Interfaces:**
- Produces:
  - `pub enum ClaudeError { Api { status: u16, error_type: String, message: String }, RateLimited { retry_after: Option<Duration> }, Overloaded, Transport(reqwest::Error), Decode { status: u16, body_snippet: String } }` (thiserror). `Api` is built from the API's error body `{"type":"error","error":{"type":"...","message":"..."}}`.
  - `pub struct RetryPolicy { pub max_attempts: u32, pub base_delay: Duration, pub max_delay: Duration }` with `Default` = `3`, `500 ms`, `8 s`; `Client::with_retry_policy(self, p: RetryPolicy) -> Client`.
  - Retry on: 429, 529, 500, 502, 503, 504, and transport timeouts/connection errors. Never retry other 4xx.
  - Delay: `retry-after` header (seconds) if present and ≤ `max_delay`, else exponential `base_delay * 2^(attempt-1)` with ±20% jitter, capped at `max_delay`.
  - `impl Debug for Client` prints `Client { base_url: .., api_key: "[redacted]" }`.

- [ ] **Step 1: Write the failing tests** (use `RetryPolicy { max_attempts: 3, base_delay: 1ms, max_delay: 50ms }` to keep tests fast):
  - `bad_request_is_not_retried`: 400 with error body `invalid_request_error` / `"max_tokens: required"` → `Err(ClaudeError::Api { status: 400, error_type: "invalid_request_error", .. })`; mock `.expect(1)`.
  - `rate_limit_honours_retry_after_then_succeeds`: first response 429 with `retry-after: 0`, second 200 → `Ok`; mock hit count 2.
  - `overloaded_gives_up_after_three_attempts`: always 529 → `Err(ClaudeError::Overloaded)`; mock `.expect(3)`.
  - `persistent_429_returns_rate_limited`: always 429 → `Err(ClaudeError::RateLimited { .. })` after exactly 3 attempts.
  - `html_error_page_is_decode_error`: 502 with `<html>Bad Gateway</html>` on every attempt → `Err(ClaudeError::Decode { status: 502, .. })` after 3 attempts.
  - `debug_does_not_leak_key`: `format!("{:?}", Client::new(SecretString::from("sk-live-SECRET")))` does not contain `"sk-live-SECRET"`.
- [ ] **Step 2: Run** `cargo test -p claude-client` → new tests FAIL.
- [ ] **Step 3: Implement** `error.rs`, `retry.rs`, the retry loop in `create_message`, and the manual `Debug`. Classify non-2xx: parse error body if JSON → `Api`; 429 → `RateLimited`; 529 → `Overloaded`; non-JSON → `Decode`. Log each retry at `warn` with attempt number and status only (never headers or body).
- [ ] **Step 4: Run** `cargo test -p claude-client` → PASS (9 tests).
- [ ] **Step 5: Add** `examples/ping.rs`: reads `ANTHROPIC_API_KEY` and `CLAUDE_MODEL` from env, sends one user message `"Reply with the single word: ready"` with `max_tokens: 10`, prints the text and token usage.
- [ ] **Step 6: Verify live** (owner's machine or a shell with the key set): `cargo run -p claude-client --example ping` → prints `ready` (or similar) and a usage line. This spends a few tokens from the Max API credits.
- [ ] **Step 7: Commit** `git commit -m "feat(claude-client): typed errors, bounded retries, redaction"`.

---

### Task 4: App server: Leptos placeholder page and health endpoints

**Files:**
- Create: `crates/incisive-app/src/app.rs`, `src/health.rs`, `tests/health.rs`; Leptos config in `crates/incisive-app/Cargo.toml` (`[package.metadata.leptos]`, generated from the `cargo leptos new --git https://github.com/leptos-rs/start-axum` template, then trimmed)
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `Config::from_env()` (Task 1).
- Produces:
  - `#[async_trait] pub trait Readiness: Send + Sync + 'static { async fn db_ok(&self) -> bool; }`
  - `pub fn health_routes(r: Arc<dyn Readiness>) -> axum::Router` with `GET /healthz` → `200 "ok"` (never touches the DB) and `GET /readyz` → `200 {"db":"up"}` or `503 {"db":"down"}`; `/readyz` applies a 2 s timeout to `db_ok` and treats timeout as down.
  - `pub fn build_router(r: Arc<dyn Readiness>, leptos_options: LeptosOptions) -> axum::Router` merging health routes with the Leptos app.
  - `#[component] pub fn App() -> impl IntoView` rendering a single page with `<title>Incisive</title>`, an `<h1>` "Incisive" and a paragraph "Cut to the evidence."

- [ ] **Step 1: Write the failing tests** in `tests/health.rs` with `tower::ServiceExt::oneshot` and two fakes (`AlwaysUp`, `AlwaysDown` where `db_ok` returns `false`, and `Hangs` where `db_ok` sleeps 10 s):
  - `healthz_ok_even_when_db_down`: `AlwaysDown` → `/healthz` 200, body `"ok"`.
  - `readyz_up`: `AlwaysUp` → 200, JSON `{"db":"up"}`.
  - `readyz_down`: `AlwaysDown` → 503, JSON `{"db":"down"}`.
  - `readyz_times_out_as_down`: `Hangs` → 503 within 3 s (use `tokio::time::pause` + `advance`).
  - `home_page_shows_name_and_tagline`: GET `/` → 200 and body contains `Incisive` and `Cut to the evidence.`
- [ ] **Step 2: Run** `cargo test -p incisive-app --test health` → FAIL.
- [ ] **Step 3: Implement** `health.rs`, `app.rs`, `build_router`. In `main.rs`: init `tracing_subscriber` JSON formatter; `Config::from_env()` and on error print the `ConfigError` to stderr and `std::process::exit(1)`; build router; bind `0.0.0.0:{config.port}`. Database wiring arrives in Task 5; until then pass `AlwaysDown`-equivalent `NoDb` readiness.
- [ ] **Step 4: Run** `cargo test -p incisive-app` → PASS (all config and health tests).
- [ ] **Step 5: Verify manually** `cargo leptos watch` with dummy env values, then `curl -s localhost:8080/healthz` → `ok`; open `http://localhost:8080` → page shows "Incisive" and the tagline. Run with `ANTHROPIC_API_KEY` unset → process exits with code 1 and message naming the variable.
- [ ] **Step 6: Commit** `git commit -m "feat(app): Leptos shell, healthz/readyz"`.

---

### Task 5: Supabase connection and first migration

**Files:**
- Create: `crates/incisive-app/src/db.rs`, `migrations/0001_init.sql`, `crates/incisive-app/tests/db.rs`
- Modify: `src/main.rs`, `.github/workflows/ci.yml` (Postgres service)

**Interfaces:**
- Consumes: `Readiness` (Task 4), `Config.database_url` (Task 1).
- Produces:
  - `pub fn lazy_pool(url: &SecretString) -> Result<PgPool, sqlx::Error>` using `PgPoolOptions::new().max_connections(5).acquire_timeout(Duration::from_secs(3)).connect_lazy(..)`, so boot never blocks on the database.
  - `pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError>` running `sqlx::migrate!("../../migrations")`.
  - `pub struct PgReadiness(pub PgPool)` implementing `Readiness` via `SELECT 1`.
  - `migrations/0001_init.sql`: `CREATE EXTENSION IF NOT EXISTS vector;` and `CREATE SCHEMA IF NOT EXISTS incisive;` (tables arrive in Phase 1).

- [ ] **Step 1: Write the failing tests** in `tests/db.rs`, gated on `TEST_DATABASE_URL` (skip with a printed note if unset):
  - `migrations_apply_and_ready_is_true`: `migrate` succeeds twice in a row (idempotent); `PgReadiness(pool).db_ok().await == true`; `SELECT 1 FROM pg_extension WHERE extname = 'vector'` returns one row.
  - `unreachable_db_is_not_ready_and_does_not_panic`: `lazy_pool` with `postgres://u:p@127.0.0.1:1/x` → `Ok(pool)`; `db_ok()` returns `false` within 4 s.
- [ ] **Step 2: Run** `TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo test -p incisive-app --test db` (with a local `pgvector/pgvector:pg16` container) → FAIL.
- [ ] **Step 3: Implement** `db.rs` and the migration. In `main.rs`: create `lazy_pool`; spawn `migrate` in the background, logging success or failure at `info`/`error` (a failed migration must not stop the server; `/readyz` will report the DB state); pass `PgReadiness` to `build_router`.
- [ ] **Step 4: Add** a `services: postgres` block (`pgvector/pgvector:pg16`, password `postgres`, health check `pg_isready`) to the CI test job and set `TEST_DATABASE_URL` for `cargo test`.
- [ ] **Step 5: Run** the test command from Step 2 → PASS; push and confirm CI green.
- [ ] **Step 6: Verify against Supabase** locally: `DATABASE_URL=<Supabase session pooler URL> cargo leptos watch`; `curl -s localhost:8080/readyz` → `{"db":"up"}`; in the Supabase SQL editor, `select extname from pg_extension where extname='vector';` returns one row.
- [ ] **Step 7: Commit** `git commit -m "feat(db): Supabase pool, migrations, readiness"`.

---

### Task 6: Container image and automatic deployment to Cloud Run (Mumbai)

**Files:**
- Create: `Dockerfile`, `.dockerignore`, `.github/workflows/deploy.yml`, `infra/README.md`

**Interfaces:**
- Consumes: the app binary and site assets produced by `cargo leptos build --release` (Tasks 4–5).
- Produces: Cloud Run service `incisive-app` in `asia-south1`, image `asia-south1-docker.pkg.dev/<PROJECT_ID>/incisive/incisive-app:<git-sha>`.

- [ ] **Step 1: Write** `Dockerfile` (multi-stage): builder `rust:1-bookworm` installs `cargo-leptos` and the `wasm32-unknown-unknown` target, runs `cargo leptos build --release`; runtime `gcr.io/distroless/cc-debian12:nonroot` copies the server binary and `target/site`, sets `LEPTOS_SITE_ROOT=site`, `LEPTOS_SITE_ADDR=0.0.0.0:8080`, `APP_ENV=prod`, exposes 8080. `.dockerignore` excludes `target/`, `.git/`, `*.md` except `migrations/`.
- [ ] **Step 2: Verify the image locally:** `docker build -t incisive-app .` succeeds; `docker run --rm -p 8080:8080 -e ANTHROPIC_API_KEY=x -e CLAUDE_MODEL=x -e DATABASE_URL=postgres://u:p@127.0.0.1:1/x incisive-app`; `curl -s localhost:8080/healthz` → `ok`; `curl -s -o /dev/null -w '%{http_code}' localhost:8080/readyz` → `503`.
- [ ] **Step 3: Write** `infra/README.md` with the exact one-time `gcloud` commands (owner or implementer with owner present):
  - `gcloud artifacts repositories create incisive --repository-format=docker --location=asia-south1`
  - Secrets: `gcloud secrets create anthropic-api-key` and `database-url`, values added via `gcloud secrets versions add <name> --data-file=-` (typed, never committed)
  - Service account `incisive-run@<PROJECT_ID>.iam.gserviceaccount.com` with `roles/secretmanager.secretAccessor`
  - Deployer service account `incisive-deploy@…` with `roles/run.admin`, `roles/artifactregistry.writer`, `roles/iam.serviceAccountUser`, bound to a Workload Identity Pool + GitHub OIDC provider restricted to this repository
  - Budget: `gcloud billing budgets create --billing-account=<ID> --display-name="incisive" --budget-amount=500INR --threshold-rule=percent=0.5 --threshold-rule=percent=0.9 --threshold-rule=percent=1.0` (the amount's currency must match the billing account's; if the account bills in USD, use the USD equivalent of ₹500)
- [ ] **Step 4: Run** the README commands; record `PROJECT_ID`, `WIF_PROVIDER` and `DEPLOY_SA` as GitHub repository variables (not secrets; they are not sensitive).
- [ ] **Step 5: Write** `.github/workflows/deploy.yml`, triggered on push to `main` after CI passes (`workflow_run` on CI, `conclusion == success`): `google-github-actions/auth` (WIF), `setup-gcloud`, `gcloud auth configure-docker asia-south1-docker.pkg.dev`, build and push tagged `${{ github.sha }}`, then:
  ```
  gcloud run deploy incisive-app --image=<image>:${{ github.sha }} --region=asia-south1 \
    --service-account=incisive-run@$PROJECT_ID.iam.gserviceaccount.com \
    --set-secrets=ANTHROPIC_API_KEY=anthropic-api-key:latest,DATABASE_URL=database-url:latest \
    --set-env-vars=APP_ENV=prod,CLAUDE_MODEL=<model id chosen by owner> \
    --timeout=3600 --min-instances=0 --max-instances=3 --cpu=1 --memory=512Mi \
    --cpu-throttling --allow-unauthenticated
  ```
  (`--allow-unauthenticated` is needed so browsers can reach the login page that Phase 1 adds; Phase 0 exposes only the placeholder page and health checks.)
- [ ] **Step 6: Verify deployment:** merge to `main`; both workflows green; `curl -s https://<service-url>/healthz` → `ok`; `curl -s https://<service-url>/readyz` → `{"db":"up"}`; the home page shows "Incisive" and "Cut to the evidence."; Cloud Run console shows region `asia-south1`, timeout `3600s`, max instances `3`.
- [ ] **Step 7: Verify secrets hygiene:** in Cloud Run logs for the deployment, search for `sk-ant` → no results; `gcloud run services describe incisive-app --region=asia-south1` shows secrets as Secret Manager references, not values.
- [ ] **Step 8: Commit** `git commit -m "ci: container build and Cloud Run deploy (asia-south1)"`.

---

## Done when

- `https://<service-url>/` shows the Incisive placeholder in a browser; `/readyz` reports the Supabase database up.
- Every push runs fmt, clippy, tests and `cargo audit`; every green merge to `main` deploys automatically.
- `cargo run -p claude-client --example ping` gets a reply from Claude using the Max API credits.
- The billing budget alert exists at ₹500/month.

**Next plan:** Phase 1, the Rust core: invites and passkey login, roles, the medical-only gate, the de-identification filter, per-user spending caps, and the hash-chained audit log.
