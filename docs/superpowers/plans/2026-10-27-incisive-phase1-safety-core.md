# Incisive Phase 1 (Safety Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every question a signed-in user submits passes, in order, through verified identity → patient-identifier filter → medical-only gate → spending check, with every step written to a tamper-evident audit log; the owner can try it in a simple question box and review it on an Activity page.

**Architecture:** All logic lives in the Rust `incisive-app` crate as small, independently testable modules (`identity`, `deid`, `gate`, `spend`, `audit`, `ask`), composed by one pipeline in `ask.rs` behind `POST /api/ask`. Login is Google's Identity-Aware Proxy (already live); the app verifies IAP's signed JWT on every request and maps the email to a row in `incisive.users`. The gate is one cheap Claude call (gate model) through the existing `claude-client`. No agent yet — accepted questions get a fixed "accepted for the evidence agent (Phase 2)" response.

**Tech Stack:** Rust (edition 2024), axum 0.8, Leptos 0.8, sqlx/Postgres (Supabase), `claude-client` (Phase 0), `jsonwebtoken` (ES256 JWT verification), `sha2` + `hex` (hash chain), `regex`, `chrono` (Asia/Kolkata day boundaries via `chrono-tz`), wiremock (tests).

**Spec:** `docs/superpowers/specs/surgeon-evidence-sandbox-design.md` (sections 2, 4 "Guardrails", 6, 7). Decision 7 (27 Oct 2026) replaces the spec's invites/passkeys with Google sign-in (IAP); roles, caps and the rest of section 2 still apply.

## Global Constraints

- Pipeline order is fixed: **identity → identifier filter → medical gate → spending check → proceed**. The identifier filter runs before any network call, so identifiers never leave the app.
- No identifiable patient data is ever sent to Claude, stored, or logged. Blocked text is **not** stored in the audit log — only the finding kinds (e.g. `phone`, `aadhaar`) and their character positions.
- The Claude API key and database URL never appear in logs, `Debug`, the browser, or the audit log.
- IAP JWT: header `x-goog-iap-jwt-assertion`, algorithm ES256, keys from `https://www.gstatic.com/iap/verify/public_key-jwk`, issuer `https://cloud.google.com/iap`, audience = env `IAP_AUDIENCE` (copied from the IAP console's "Get JWT audience code"). Unverified identity headers (`x-goog-authenticated-user-email`) are never trusted.
- In `APP_ENV=prod` there is no identity bypass of any kind. A dev-only header (`x-dev-user-email`) is honoured only when `APP_ENV=dev`.
- Gate policy (owner decision): **allow** clinical medicine, surgery, clinical research, academic surgery (papers, journal club, teaching, guidelines, surgical training); **refuse** pure administration, billing/coding, finance, and non-medical topics; **unclear** → one clarifying prompt, not a refusal.
- Identifier policy (owner decision): ages, sex, and relative clinical time ("post-op day 3", "6 weeks after surgery") are **allowed**; exact calendar dates, names with titles or patient context, UHID/IP/MRN-style IDs, Aadhaar, PAN, phone numbers, email addresses and street addresses are **blocked**.
- Default caps (owner decision, editable): owner ₹1,500/day and no personal monthly cap; surgeon ₹300/day and ₹3,000/month; fellow ₹150/day and ₹1,500/month; global monthly cap from env `GLOBAL_MONTHLY_CAP_INR`. Alerts at 80%. "Day" and "month" are Asia/Kolkata calendar periods.
- Money is stored as integer **paise** (`i64`); never floats in the database.
- Model IDs are configuration: `CLAUDE_MODEL` (agent, Phase 2) and `GATE_MODEL` (gate). Prices are configuration: env `MODEL_PRICES` (JSON, USD per million input/output tokens per model) and `USD_INR`.
- Every CI build still runs fmt, clippy `-D warnings`, tests (with the pgvector Postgres service), `cargo audit --ignore RUSTSEC-2023-0071`, and the container build + smoke test.
- UI copy uses the product name **Incisive**; error text is plain and tells the user what to do next.

## Review Focus

1. **IAP header present but forged or for another app** (wrong audience, expired, signed by a non-Google key, `alg: none`): request is rejected with 401 and an audit entry `identity_rejected`; never treated as signed in. Test: Task 1.
2. **Identifier hiding in ordinary clinical text** — Aadhaar with spaces or hyphens, +91 phones, `dd/mm/yyyy` dates, "Mrs. Lakshmi", "UHID: MH-2231" — must be caught, while "52 y/o M, T3N1, POD 3, Hb 9.8, 120 mg" must pass. Test: Task 2 table.
3. **Gate model returns junk or fails** (non-JSON, unknown label, timeout, 529): the gate fails **closed** for that request with "couldn't check this question, please try again" — never silently treats it as medical — and the failure is audited. Test: Task 3.
4. **Two requests at once push a user over the cap** (double-click, two tabs): the cap check-and-reserve is atomic, so at most one passes when only one fits. Test: Task 4.
5. **Concurrent audit writes** (two users at once): the chain never forks — every entry's `prev_hash` equals the previous entry's `hash`, and `verify_chain` stays valid. Test: Task 5.

---

## Build process (owner requirement, 27 Oct 2026)

Execution: subagent-driven (fresh implementer per task, independent reviewer per task, fix loop until clean, whole-phase review at the end). The seven tasks are grouped into five **stages**; each stage ends with a written report and a message to the owner before the next stage starts.

| Stage | Tasks | What it delivers |
|---|---|---|
| A. Identity | 1 | Verified Google identity on every request; users table; owner set up |
| B. Safety filters | 2, 3 | Local patient-identifier filter; medical-only gate (fails closed) |
| C. Money and records | 4, 5 | Spending caps in paise; hash-chained audit log |
| D. Ask pipeline and screens | 6 | Question box, Activity page, all checks joined in order |
| E. Production wiring and final review | 7 + whole-phase review | Live settings, CI smoke checks, independent review of the whole phase |

**Per task (step):** implementer (tests first) → task review (spec compliance + code quality) → fix rounds with scoped re-review until clean → task recorded in the ledger.

**Per stage (gate):** all stage tasks clean → branch pushed → GitHub CI green (fmt, clippy, tests with Postgres+pgvector, audit, container build including the browser/WASM build, smoke test) → stage report written and committed → owner message sent.

**Stage reports:** `docs/build-log/phase1/stage-<letter>-<name>.md` in the repository (permanent, versioned with the code), and the same content appended to the Project document `plans/incisive-phase1-build-log.md`. Each report uses this template:

```
# Phase 1 · Stage <X>: <name>
Date · Branch · Commits <base>..<head> · CI run <id> <result>

## What was built (plain English)
## How to see it / what changes for the user
## Tasks
| Task | Implementer status | Review verdict | Fix rounds | Commits |
## Tests
New tests (names, counts) · full-suite result · CI result
## Review findings
Important/critical findings and how each was resolved; minor findings deferred
## Decisions made during the stage (rulings)
Each with: what, why, cost if wrong
## Known limitations / follow-ups
## Next stage
```

---

## File structure

```
crates/incisive-app/
  src/config.rs        + IAP_AUDIENCE, OWNER_EMAIL, GATE_MODEL, MODEL_PRICES, USD_INR, GLOBAL_MONTHLY_CAP_INR
  src/identity.rs      IAP JWT verification, CurrentUser extractor, users repo     (Task 1)
  src/deid.rs          pure identifier scanner (no IO)                              (Task 2)
  src/gate.rs          rules prefilter + Classifier trait + Claude classifier       (Task 3)
  src/spend.rs         pricing, cost in paise, caps, reserve/settle                 (Task 4)
  src/audit.rs         hash-chained append, verify_chain, list                      (Task 5)
  src/ask.rs           pipeline + POST /api/ask                                     (Task 6)
  src/app.rs           Ask page, Activity page (owner), signed-in banner            (Task 6)
  tests/identity.rs, tests/deid.rs, tests/gate.rs, tests/spend.rs, tests/audit.rs, tests/ask.rs
migrations/
  0002_users.sql  0003_spend.sql  0004_audit.sql
.github/workflows/deploy.yml   + new env vars                                       (Task 7)
infra/README.md                + Phase 1 settings                                   (Task 7)
```

`main.rs` builds one `AppState` (defined in Task 6) and passes it to `build_router`; the Claude client lives inside `ClaudeClassifier`.

---

### Task 1: Identity from Google sign-in

**Files:**
- Create: `src/identity.rs`, `migrations/0002_users.sql`, `tests/identity.rs`
- Modify: `src/config.rs` (add `iap_audience: String`, `owner_email: String`), `src/lib.rs`, `src/main.rs`, `Cargo.toml` (`jsonwebtoken`, `reqwest` with rustls for JWK fetch, `serde`)

**Interfaces:**
- Produces:
  - Migration `0002_users.sql`: `CREATE TABLE incisive.users (id BIGSERIAL PRIMARY KEY, email TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL, role TEXT NOT NULL CHECK (role IN ('owner','surgeon','fellow')), daily_cap_paise BIGINT NOT NULL, monthly_cap_paise BIGINT, active BOOLEAN NOT NULL DEFAULT TRUE, created_at TIMESTAMPTZ NOT NULL DEFAULT now());` (emails stored lower-case).
  - `pub enum Role { Owner, Surgeon, Fellow }`
  - `pub struct User { pub id: i64, pub email: String, pub display_name: String, pub role: Role, pub daily_cap_paise: i64, pub monthly_cap_paise: Option<i64>, pub active: bool }`
  - `pub struct IapVerifier` with `pub fn new(audience: String, keys: JwkSource) -> Self`, `pub async fn verify(&self, jwt: &str) -> Result<VerifiedIdentity, IdentityError>`; `pub enum JwkSource { Remote(Url), Static(jsonwebtoken::jwk::JwkSet) }` (Static for tests). Remote keys cached in memory, refreshed on unknown `kid` at most once per 5 minutes.
  - `pub struct VerifiedIdentity { pub email: String }` (lower-cased, from claim `email`).
  - `pub enum IdentityError { Missing, Invalid(&'static str), KeysUnavailable }`
  - `pub async fn find_user(pool: &PgPool, email: &str) -> sqlx::Result<Option<User>>`
  - `pub async fn ensure_owner(pool: &PgPool, email: &str, display_name: &str) -> sqlx::Result<User>` — upsert: role owner, daily ₹1,500 (150_000 paise), monthly NULL.
  - Axum extractor `pub struct CurrentUser(pub User)`: in prod reads `x-goog-iap-jwt-assertion` → verify → `find_user`; in dev reads `x-dev-user-email` instead. Rejections: no/invalid JWT → 401 JSON `{"error":"not_signed_in"}`; verified but no row or `active=false` → 403 JSON `{"error":"not_set_up","message":"Your Google account is allowed in, but you haven't been set up in Incisive yet. Ask the owner to add you."}`.
- Config: `IAP_AUDIENCE` required in prod, optional in dev; `OWNER_EMAIL` required.
- At startup (after migrations succeed) `main.rs` calls `ensure_owner(OWNER_EMAIL, "Owner")`.

- [ ] **Step 1: Write failing tests** in `tests/identity.rs` using a test ES256 keypair and `JwkSource::Static`:
  - `valid_token_yields_email` (lower-cases `Manj@Example.com`)
  - `wrong_audience_rejected`, `expired_rejected`, `wrong_issuer_rejected`, `unknown_kid_rejected`, `alg_none_rejected`, `hs256_with_public_key_as_secret_rejected` → each `IdentityError::Invalid(_)`
  - Router-level (dev mode off): `missing_header_is_401`, `verified_but_unknown_user_is_403_not_set_up`, `inactive_user_is_403`, `unverified_email_header_ignored` (only `x-goog-authenticated-user-email` present → 401)
  - Dev mode: `dev_header_works_only_in_dev` (same request in prod config → 401)
  - DB-gated (`TEST_DATABASE_URL`): `ensure_owner_is_idempotent` (two calls → one row, role owner, cap 150000)
- [ ] **Step 2: Run** `cargo test -p incisive-app --test identity` → FAIL.
- [ ] **Step 3: Implement** `identity.rs`, migration, config fields, extractor; `Validation` must set `algorithms = [ES256]`, `iss`, `aud`, `exp` required, 30 s leeway.
- [ ] **Step 4: Run** the identity tests and the full workspace suite → PASS.
- [ ] **Step 5: Commit** `feat(identity): verify IAP sign-in, users table, owner bootstrap`.

---

### Task 2: Patient-identifier filter

**Files:**
- Create: `src/deid.rs`, `tests/deid.rs`
- Modify: `src/lib.rs` (module is NOT `ssr`-gated: pure Rust, usable later in the browser), `Cargo.toml` (`regex`, `once_cell` or std `LazyLock`)

**Interfaces:**
- Produces:
  - `pub enum FindingKind { Aadhaar, Pan, Phone, Email, CalendarDate, HospitalId, PersonName, Address }` (serde snake_case)
  - `pub struct Finding { pub kind: FindingKind, pub start: usize, pub end: usize }` (byte offsets into the input)
  - `pub fn scan(text: &str) -> Vec<Finding>` — sorted by `start`, overlapping findings merged.
  - `pub fn describe(kind: FindingKind) -> &'static str` — user-facing label, e.g. `"phone number"`.
- Rules (implementer chooses regexes; tests define behaviour):
  - Aadhaar: 12 digits as `dddd dddd dddd`, `dddd-dddd-dddd` or contiguous, first digit 2–9.
  - PAN: `AAAAA9999A`.
  - Phone: Indian mobiles (`+91`/`0`/none, 10 digits starting 6–9, with spaces/hyphens) and landlines with STD codes.
  - Email: standard address pattern.
  - CalendarDate: `dd/mm/yyyy`, `dd-mm-yyyy`, `dd.mm.yy`, `yyyy-mm-dd`, `12 March 2024`, `March 12, 2024`, `12th Mar`. Not years alone ("in 2019"), not relative time.
  - HospitalId: a label (`UHID`, `MRN`, `IP no`, `IPD`, `OP no`, `Reg no`, `Hospital no`, `Patient ID`, case-insensitive, optional `.`/`:`/`#`) followed by an alphanumeric ID.
  - PersonName: title (`Mr`, `Mrs`, `Ms`, `Miss`, `Shri`, `Smt`, `Kumari`, `Master`, `Baby of`, optional `.`) followed by a capitalised word; or `patient`/`pt`/`name`/`named`/`called` followed by a capitalised word that is not a medical term (small built-in allow-list: e.g. Crohn, Whipple, Hartmann, Nissen, Roux, Child-Pugh).
  - Address: `door/flat/house no`, `#12`, `cross`, `main road`, `layout`, `nagar` with a number, or a 6-digit PIN code.
- Must NOT flag: ages (`52`, `52 yo`, `52 y/o`, `52-year-old`), sex (`M`, `F`, `male`), relative time (`POD 3`, `day 5`, `6 weeks post-op`), lab values and units (`Hb 9.8 g/dL`, `CEA 12.4`, `120 mg`), TNM/stage (`T3N1M0`), scores (`ASA 3`, `Clavien-Dindo IIIa`), years alone, doses, percentages, eponyms (Whipple, Hartmann, Crohn).

- [ ] **Step 1: Write failing tests** in `tests/deid.rs` as one table-driven test of ≥ 60 cases (input → expected kinds), at least 25 must-block and 25 must-pass, including all of: `"Aadhaar 2345 6789 0123"`, `"ph +91 98450 12345"`, `"call 080-2502 4444"`, `"DOB 12/03/1971"`, `"seen on 2024-03-12"`, `"Mrs. Lakshmi, 52F"`, `"patient Ramesh had"`, `"UHID: MH-2231"`, `"IP no 44718"`, `"#12, 4th Cross, Indiranagar 560038"`, `"PAN ABCDE1234F"`; and must-pass `"52 y/o M, T3N1M0 rectal ca, POD 3, Hb 9.8, CEA 12.4"`, `"Whipple vs PPPD in 2019 trial"`, `"Clavien-Dindo IIIa leak on day 5"`, `"ASA 3, BMI 31"`, `"120 mg bd for 6 weeks"`. Plus `offsets_point_at_the_match` and `overlaps_are_merged`.
- [ ] **Step 2: Run** `cargo test -p incisive-app --test deid` → FAIL.
- [ ] **Step 3: Implement** `deid.rs` (regexes compiled once).
- [ ] **Step 4: Run** → PASS; also `cargo build -p incisive-app --no-default-features --features hydrate --lib --target wasm32-unknown-unknown` is NOT required locally (no wasm target here); keep `deid.rs` free of `ssr`-only dependencies.
- [ ] **Step 5: Commit** `feat(deid): local patient-identifier scanner`.

---

### Task 3: Medical-only gate

**Files:**
- Create: `src/gate.rs`, `tests/gate.rs`
- Modify: `src/config.rs` (add `gate_model: String`, default `claude-haiku-5-5` if unset)

**Interfaces:**
- Consumes: `claude_client::{Client, MessagesRequest, Message, Role, ContentBlock, MessagesResponse, Usage}`.
- Produces:
  - `pub enum Verdict { Medical, NotMedical, Unclear }` (serde snake_case)
  - `pub struct GateDecision { pub verdict: Verdict, pub reason: String, pub by_rule: bool, pub usage: Option<claude_client::Usage>, pub model: Option<String> }`
  - `pub enum GateError { Unavailable, BadResponse }`
  - `#[async_trait] pub trait Classifier: Send + Sync { async fn classify(&self, question: &str) -> Result<GateDecision, GateError>; }`
  - `pub fn rule_prefilter(question: &str) -> Option<GateDecision>` — returns `NotMedical` (by_rule) for empty/whitespace, > 4,000 chars ("too long" reason), or obvious non-medical keywords only when no medical term is present (small lists; e.g. "bitcoin", "stock price", "write my essay", "cricket score"); returns `Medical` (by_rule) when the text contains ≥ 2 terms from a small surgical/medical lexicon (e.g. resection, anastomosis, laparoscopic, robotic, cholecystectomy, hernia, carcinoma, RCT, meta-analysis, guideline); otherwise `None`.
  - `pub struct ClaudeClassifier { client: Arc<Client>, model: String }` implementing `Classifier`: one `create_message` call, `max_tokens: 150`, temperature unset, system prompt containing the Gate policy from Global Constraints verbatim, asking for **only** JSON `{"verdict":"medical|not_medical|unclear","reason":"<one sentence>"}`. Parse strictly: trim, accept a single JSON object (optionally inside a ```json fence); anything else → `GateError::BadResponse`.
  - `pub async fn check(question: &str, classifier: &dyn Classifier) -> Result<GateDecision, GateError>` — rules first, else classifier.
  - User-facing messages: `NotMedical` → `"Incisive only answers medical and surgical questions. <reason>"`; `Unclear` → `"Could you say a little more about the clinical question? <reason>"`; `GateError` → `"I couldn't check this question just now. Please try again in a moment."`

- [ ] **Step 1: Write failing tests** in `tests/gate.rs`: rule-prefilter table (≥ 15 cases, including "ICG in robotic low anterior resection — evidence for leak?" → Medical by rule, "bitcoin price tomorrow" → NotMedical by rule, "hello" → None); `ClaudeClassifier` against wiremock: `parses_medical`, `parses_fenced_json`, `unknown_label_is_bad_response`, `prose_answer_is_bad_response`, `overloaded_is_unavailable` (529 ×3), and `request_uses_gate_model_and_150_tokens` (body match); `check_uses_rules_before_model` (mock with `.expect(0)` for a rule-decided question); `gate_fails_closed` (classifier error → `Err`, never `Medical`).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(gate): medical-only gate with rules and cheap model`.

---

### Task 4: Spending caps

**Files:**
- Create: `src/spend.rs`, `migrations/0003_spend.sql`, `tests/spend.rs`
- Modify: `src/config.rs` (`model_prices: Prices` parsed from `MODEL_PRICES`, `usd_inr: f64` from `USD_INR`, `global_monthly_cap_paise: i64` from `GLOBAL_MONTHLY_CAP_INR`; all required in prod; dev defaults allowed)

**Interfaces:**
- Produces:
  - Migration `0003_spend.sql`: `CREATE TABLE incisive.spend (id BIGSERIAL PRIMARY KEY, user_id BIGINT NOT NULL REFERENCES incisive.users(id), at TIMESTAMPTZ NOT NULL DEFAULT now(), kind TEXT NOT NULL CHECK (kind IN ('reservation','charge')), purpose TEXT NOT NULL, model TEXT, input_tokens INT, output_tokens INT, paise BIGINT NOT NULL, request_id UUID NOT NULL);` plus index on `(user_id, at)`.
  - `pub struct Prices(HashMap<String, ModelPrice>)`; `pub struct ModelPrice { pub input_usd_per_mtok: f64, pub output_usd_per_mtok: f64 }`; `MODEL_PRICES` JSON shape `{"<model id>": {"input": <usd>, "output": <usd>}}`; parse errors name the variable.
  - `pub fn cost_paise(prices: &Prices, usd_inr: f64, model: &str, usage: &Usage) -> Result<i64, SpendError>` — rounds **up** to the next paisa; unknown model → `SpendError::UnknownModel(model)` (fails closed).
  - `pub struct CapsConfig { pub global_monthly_paise: i64 }`
  - `pub enum CapCheck { Ok { alert: Option<String> }, Blocked { message: String } }`
  - `pub async fn reserve(pool: &PgPool, user: &User, caps: &CapsConfig, estimate_paise: i64, request_id: Uuid, purpose: &str) -> sqlx::Result<CapCheck>` — in one transaction with `SELECT pg_advisory_xact_lock(user.id)` and a global lock key: sums the user's reservations+charges for the current Asia/Kolkata day and month and the global month; blocks if `spent + estimate` exceeds any applicable cap; otherwise inserts a `reservation` row. Alert text when any total after reservation ≥ 80% of its cap.
  - `pub async fn settle(pool: &PgPool, user_id: i64, request_id: Uuid, model: Option<&str>, usage: Option<&Usage>, paise: i64) -> sqlx::Result<()>` — deletes that request's reservation and inserts the `charge` (a zero-paise charge when no model call happened), atomically.
  - `pub fn default_caps(role: Role) -> (i64, Option<i64>)` — owner (150_000, None), surgeon (30_000, Some(300_000)), fellow (15_000, Some(150_000)).
  - Blocked messages: `"You've reached today's Incisive limit (₹X). It resets at midnight IST."` / `"…this month's limit…"` / `"Incisive has reached its overall monthly budget. Please tell the owner."`
- Phase 1 estimate for a gate call: a fixed 1,000 input + 150 output tokens priced with the gate model.

- [ ] **Step 1: Write failing tests** in `tests/spend.rs`: `cost_rounds_up_to_paisa`, `unknown_model_fails_closed`, `prices_json_parse_errors_name_variable`; DB-gated: `blocks_when_daily_cap_would_be_exceeded`, `month_and_day_use_ist_boundaries` (insert rows at 18:29 UTC and 18:31 UTC on the same UTC date and assert they fall on different IST days), `alert_at_80_percent`, `settle_replaces_reservation`, `global_cap_blocks_everyone`, and `concurrent_reservations_do_not_overshoot` (two tasks reserve 60% each of a cap simultaneously → exactly one `Ok`).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(spend): per-user and global caps in paise`.

---

### Task 5: Tamper-evident audit log

**Files:**
- Create: `src/audit.rs`, `migrations/0004_audit.sql`, `tests/audit.rs`
- Modify: `Cargo.toml` (`sha2`, `hex`, `uuid` with `v4` + `serde`)

**Interfaces:**
- Produces:
  - Migration `0004_audit.sql`: `CREATE TABLE incisive.audit (seq BIGINT PRIMARY KEY, at TIMESTAMPTZ NOT NULL, request_id UUID, user_id BIGINT, event TEXT NOT NULL, detail JSONB NOT NULL, prev_hash TEXT NOT NULL, hash TEXT NOT NULL UNIQUE);` and `REVOKE UPDATE, DELETE ON incisive.audit FROM PUBLIC;` (app role keeps INSERT/SELECT only where Supabase permits; document in README if role separation needs the owner).
  - `pub enum Event { IdentityRejected, DeidBlocked, GateDecided, GateFailed, CapBlocked, Accepted, Charged }` (serde snake_case)
  - `pub struct Entry { pub seq: i64, pub at: DateTime<Utc>, pub request_id: Option<Uuid>, pub user_id: Option<i64>, pub event: Event, pub detail: serde_json::Value, pub prev_hash: String, pub hash: String }`
  - `pub fn entry_hash(prev_hash: &str, seq: i64, at: DateTime<Utc>, request_id: Option<Uuid>, user_id: Option<i64>, event: Event, detail: &serde_json::Value) -> String` — lowercase hex SHA-256 over a canonical byte string: fields joined by `\x1f`, `at` as RFC 3339 with microseconds, `detail` serialised with sorted keys. Genesis `prev_hash` = 64 zeros.
  - `pub async fn append(pool: &PgPool, request_id: Option<Uuid>, user_id: Option<i64>, event: Event, detail: serde_json::Value) -> sqlx::Result<Entry>` — transaction with `pg_advisory_xact_lock(<fixed audit key>)`, reads the last row, computes `seq+1` and hash, inserts.
  - `pub enum ChainStatus { Valid { entries: i64 }, Broken { at_seq: i64 } }`
  - `pub async fn verify_chain(pool: &PgPool) -> sqlx::Result<ChainStatus>` — streams in `seq` order, recomputes every hash and link.
  - `pub async fn recent(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Entry>>`
- Detail rules: never include question text for `DeidBlocked` (only kinds + offsets + length); for `GateDecided` include verdict, reason, by_rule, model, tokens; for `Accepted` include question length and SHA-256 of the question (not the text); for `Charged` include model, tokens, paise.

- [ ] **Step 1: Write failing tests** in `tests/audit.rs`: pure: `hash_is_stable_and_order_sensitive`, `detail_key_order_does_not_change_hash`; DB-gated: `chain_links_and_verifies`, `edited_detail_breaks_chain_at_that_seq` (UPDATE via a superuser test connection, then `Broken { at_seq }`), `deleted_middle_row_breaks_chain`, `concurrent_appends_never_fork` (50 concurrent appends → 50 sequential seqs, chain Valid).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(audit): hash-chained append-only log`.

---

### Task 6: The ask pipeline, question box and Activity page

**Files:**
- Create: `src/ask.rs`, `tests/ask.rs`
- Modify: `src/server.rs` (routes + `AppState`), `src/main.rs`, `src/app.rs`

**Interfaces:**
- Consumes: Tasks 1–5 exactly as defined above.
- Produces:
  - `pub struct AppState { pub pool: PgPool, pub verifier: Arc<IapVerifier>, pub gate: Arc<dyn Classifier>, pub prices: Prices, pub usd_inr: f64, pub caps: CapsConfig, pub gate_model: String, pub dev_mode: bool }`
  - `POST /api/ask` (JSON `{"question": "<text>"}`) → JSON `AskOutcome`:
    - `{"status":"blocked_identifiers","findings":[{"kind":"phone","label":"phone number","start":..,"end":..}],"message":"…remove and resend"}`
    - `{"status":"not_medical","message":…}` / `{"status":"unclear","message":…}` / `{"status":"gate_unavailable","message":…}`
    - `{"status":"over_cap","message":…}`
    - `{"status":"accepted","message":"Accepted. The evidence agent arrives in Phase 2.","alert":null|"…80%…"}`
  - Pipeline (in this order, each step audited with one shared `request_id`): `CurrentUser` → `deid::scan` (block → audit `DeidBlocked`, return) → `spend::reserve` for the gate estimate (block → audit `CapBlocked`, return) → `gate::check` (error → audit `GateFailed`, settle reservation to 0, return; verdict → audit `GateDecided`) → if a model call happened, `spend::settle` with real usage + audit `Charged`, else settle to 0 → if Medical, audit `Accepted`.
  - `GET /api/activity?limit=50` (owner only, else 403) → recent entries + `ChainStatus`.
  - `GET /api/me` → `{"display_name","role","today_paise","daily_cap_paise"}`.
  - UI (Leptos, SSR + hydrate): banner "Signed in as <name>"; Ask page: textarea, Send button, result area — red box listing each finding label for `blocked_identifiers`, amber for `not_medical`/`unclear`/`over_cap`/`gate_unavailable`, green for `accepted`, plus the alert line if present; Activity page (owner): table of time (IST), user email, event, short detail, and a chain badge "Chain verified ✓ (N entries)" or "Chain broken at #N".
- The question text itself is never stored anywhere in Phase 1.

- [ ] **Step 1: Write failing tests** in `tests/ask.rs` (router via `oneshot`, dev identity header, fake `Classifier`, DB-gated):
  - `identifiers_blocked_before_gate` (fake classifier `.expect(0)` semantics: panics if called) → `blocked_identifiers`, audit has `DeidBlocked` without the text
  - `non_medical_refused_and_audited`, `unclear_asks_for_more`, `gate_failure_fails_closed`
  - `over_cap_blocked_before_gate` (user with cap 0)
  - `accepted_path_audits_gate_charge_accepted_in_order` and records a `charge` row with the fake usage
  - `question_text_never_persisted` (scan all `audit.detail` and `spend` rows for a unique marker string from the question)
  - `activity_is_owner_only`
  - `prod_mode_ignores_dev_header`
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement** pipeline, routes and pages. **Step 4: Run** the full workspace suite → PASS.
- [ ] **Step 5: Manual check** with a local Postgres and `APP_ENV=dev`: owner bootstrapped; ask "52 y/o M, robotic LAR, ICG for leak?" → accepted (rules); "Mrs. Lakshmi ph 9845012345" → red box listing a name and a phone; "best stocks today" → refused; Activity shows the entries with "Chain verified ✓". Record outputs in the report.
- [ ] **Step 6: Commit** `feat(ask): safety pipeline, question box, activity log`.

---

### Task 7: Production wiring

**Files:**
- Modify: `.github/workflows/deploy.yml`, `.github/workflows/ci.yml` (smoke test env), `infra/README.md`, `Dockerfile` only if needed

**Interfaces:**
- New Cloud Run env vars set by deploy from GitHub repository variables: `IAP_AUDIENCE`, `OWNER_EMAIL`, `GATE_MODEL`, `MODEL_PRICES`, `USD_INR`, `GLOBAL_MONTHLY_CAP_INR` (none are secrets). The deploy's fail-fast check is extended to all of them.
- CI container smoke test: start the image with `APP_ENV=dev` and dummy values for the new variables; `/health` → `ok` still; `POST /api/ask` without identity → 401.
- `infra/README.md` new section 9: where to get `IAP_AUDIENCE` (IAP console → incisive-app ⋮ → "Get JWT audience code"), how to set `MODEL_PRICES` from Anthropic's pricing page (example shape only, no numbers invented), suggested `USD_INR`, and `GLOBAL_MONTHLY_CAP_INR` as about 90% of the owner's monthly Max API credit; how to add a colleague (test user + IAP policy + a row in `incisive.users` with role and caps — SQL snippet).

- [ ] **Step 1:** Update workflows and README; YAML parses; `bash -n` on changed shell steps.
- [ ] **Step 2:** Push the branch; GitHub CI (including container smoke) green.
- [ ] **Step 3:** Owner sets the six repository variables (deferred to owner; controller lists exact names and where each value comes from).
- [ ] **Step 4:** After merge to `main` and deploy: in a signed-in browser, the Ask page works end to end against production; Activity shows the chain verified.
- [ ] **Step 5: Commit** `deploy: Phase 1 settings`.

---

## Done when

- A signed-in owner can ask questions on the live site; identifiers are blocked locally with clear feedback; off-topic questions are refused; medical questions are accepted.
- Every step is in the audit log, the chain verifies, and no question text or identifier is stored anywhere.
- Spending is recorded in paise against the user, caps block cleanly, and the 80% alert appears.
- All CI checks, including the container smoke test, are green.

**Next plan:** Phase 2, the evidence agent (PubMed / Europe PMC / Unpaywall tools, screening, appraisal, citation verifier), plugged in where Phase 1 now returns "accepted".
