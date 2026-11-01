# Incisive memory layer: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep every Full run's answer and Expert Opinion as a permanent, de-identified, versioned evidence record grouped by topic, with a growing era ledger, an owner expert layer, and a gate that offers a stored answer before a new run.

**Architecture:** Additive Postgres tables (migration 0013) and a new `src/memory/` module: pure `extract` and `matcher`, SQL in `store`, a queue drained by a background loop after the temporal job finishes, an HTTP `embed` client to the owner's Ollama VPS, then owner Topics pages and an Ask-page gate. Everything behind `MEMORY=off|on`. See ADR-001 (`.10x/adrs/001-memory-layer-verbatim-postgres.md`).

**Tech Stack:** Rust 2024, Leptos + Axum, sqlx 0.8 (plain `sqlx::query`), Postgres 16 + pgvector, reqwest 0.13 (`ssr` feature), wiremock 0.6 for tests, Ollama `embeddinggemma` (768-dim).

**Spec:** `docs/superpowers/specs/2026-11-01-incisive-memory-layer-design.md`

## Global Constraints

- All paths below are relative to `crates/incisive-app/` unless they start with `migrations/`, `docs/`, `.github/` or `infra/`.
- The run engine is frozen: no change to prompts, schemas, money or time logic, `run_answers`, or AnswerV3 bytes. Memory code may only *read* runs and be *triggered after* the temporal job finishes.
- No memory step may fail, delay or alter a run. Errors become job status codes.
- Never log question, answer or board text. Job errors are short codes (`&'static str`).
- `MEMORY` accepts only `off` (default) or `on`; any other value is `ConfigError::Invalid`.
- Vector dimension is exactly 768; embedding model default `embeddinggemma`. Vectors bound as text (`'[..]'`) and cast `$n::vector`.
- Gemma endpoint: `GEMMA_URL` (e.g. `https://ollama-s9os.srv2117610.hstgr.cloud`), `GEMMA_AUTH` = `incisive:<password>` from Secret Manager secret `gemma-auth`. Never logged, never in URLs.
- Embedding HTTP timeout: 5 s in the background loop, 2 s at gate time.
- Owner-only behaviour is checked in the domain function (`user.role != Role::Owner` → `NotOwner` → 403), as in `ask.rs:646`.
- Other users never see the original question or anything identifying who asked.
- Records, verdicts and owner revisions are append-only; topics are merged by pointer (`merged_into`), never deleted.
- House process per stage: builder subagent, then an independent reviewer; `cargo fmt`, `cargo clippy` (workspace and hydrate), full test suite, release hydrate build; PR with CI green; merge only on the owner's "merge it". UI tasks start with a mockup the owner approves.
- **Sequencing:** Tasks 1–8 (M0–M3) may proceed now. Tasks 9–12 (M4–M5) start only after the temporal T4 Expert Opinion UI is merged to `main`; rebase `memory-layer` onto `main` first.

## Review Focus

1. **A stored body that isn't AnswerV3, or a temporal row with missing/old-version JSON** (backfill meets older runs): `extract` returns `Err(ExtractError::…)`, the job is `skipped` with a code, no panic. Test in Task 3.
2. **Two loop ticks or two instances claiming the same job** (Cloud Run can run up to 3 instances): jobs are claimed with `FOR UPDATE SKIP LOCKED`, so exactly one record is made. Test in Task 6.
3. **A topic merged into another, possibly twice** (A→B→C): matching and the gate resolve `merged_into` to the final topic and never offer a merged-away topic. Test in Tasks 5 and 11.
4. **The VPS answers 200 with something that isn't a 768-float vector** (Traefik HTML 404, Ollama error JSON, model not pulled): treated as an embed failure, embedding stays NULL, full-text still works. Test in Task 8.
5. **The run is deleted (365-day retention) while its memory job is pending or mid-flight**: the job row cascades away or the insert fails cleanly (job `failed`, code `run_gone`); no orphan half-record (single transaction). Test in Task 6.

---

## Stage M0: schema and switch

### Task 1: Migration 0013

**Files:**
- Create: `migrations/0013_memory.sql`
- Test: `tests/memory_schema.rs`

**Interfaces:**
- Produces: tables `incisive.topics`, `evidence_records`, `ledger_studies`, `ledger_claims`, `topic_verdicts`, `owner_revisions`, `topic_changes`, `memory_jobs`, exactly as spec §4 (columns, CHECKs, PKs, FKs). Plus:
  - `evidence_records.fts` = `GENERATED ALWAYS AS (to_tsvector('english', coalesce(question,'') || ' ' || coalesce(topic_label,'') || ' ' || coalesce(fts_extra,''))) STORED`; this needs two extra plain columns, `topic_label TEXT NOT NULL` (copied at insert) and `fts_extra TEXT NOT NULL DEFAULT ''` (verdict line + outcome names).
  - `memory_jobs.status CHECK IN ('pending','running','done','failed','skipped')`, `attempts INT NOT NULL DEFAULT 0`, `claimed_at TIMESTAMPTZ NULL`.
  - Indexes: GIN on `fts`; HNSW `vector_cosine_ops` on `evidence_records.embedding` and `topics.embedding`; B-tree on `evidence_records(topic_id)`, `evidence_records(evidence_through)`, `memory_jobs(status)` partial `WHERE status='pending'`.
  - `ALTER TABLE ... ENABLE ROW LEVEL SECURITY` on all eight tables, no policies.

- [ ] **Step 1: Write the failing tests** (`connect()` pattern from `tests/temporal_job.rs:31–45`):
  - `memory_tables_exist`: each of the eight tables is in `information_schema.tables` for schema `incisive`.
  - `record_survives_run_delete`: insert a user, a run, a topic and an evidence_record with `source_run_id = run.id`; `DELETE FROM incisive.runs WHERE id=$1`; the record still exists and `source_run_id IS NULL`.
  - `memory_job_cascades_with_run`: insert run + memory_job; delete run; the job row is gone.
  - `rls_enabled`: `relrowsecurity` is true in `pg_class` for all eight tables.
  - `verdict_check_rejects_unknown`: inserting verdict `'maybe'` fails.
- [ ] **Step 2: Run** `cargo test -p incisive-app --test memory_schema` with `TEST_DATABASE_URL` set. Expected: FAIL (relations do not exist).
- [ ] **Step 3: Write `migrations/0013_memory.sql`**, additive only, with an "additive, safe to run once via sqlx migrate" header comment in the house style.
- [ ] **Step 4: Run the tests again.** Expected: PASS. Run the full suite: no existing test changes.
- [ ] **Step 5: Commit** `feat(memory): migration 0013 memory tables`

### Task 2: `MEMORY` switch and settings

**Files:**
- Create: `src/memory/mod.rs` (module root; `pub mod settings;` now, more later), `src/memory/settings.rs`
- Modify: `src/config.rs` (new field + parsing beside `TEMPORAL`, ~:232), `src/lib.rs` (`pub mod memory;`), `src/server.rs:29` (`AppState.memory: MemorySettings`), `main.rs:151` (fill it), every test helper that builds `AppState` literally (e.g. `tests/temporal_job.rs` `fn state`) gets `memory: MemorySettings::default()`
- Test: unit tests in `src/config.rs`'s existing test module

**Interfaces:**
- Produces:
  ```rust
  pub struct MemorySettings {
      pub on: bool,
      pub gemma_url: Option<String>,          // GEMMA_URL, trailing '/' trimmed
      pub gemma_auth: Option<SecretString>,   // GEMMA_AUTH
      pub embed_model: String,                // EMBED_MODEL, default "embeddinggemma"
      pub topic_match: f32,                   // MEMORY_TOPIC_MATCH, default 0.80, range 0.0..=1.0
      pub gate_match: f32,                    // MEMORY_GATE_MATCH, default 0.85, range 0.0..=1.0
  }
  impl Default for MemorySettings   // on: false, others as above, url/auth None
  impl MemorySettings { pub fn embeddings_enabled(&self) -> bool } // url && auth both Some
  ```
  - `Config.memory: MemorySettings`.

- [ ] **Step 1: Failing tests** in the config test module:
  - `memory_defaults_off`: no vars → `!cfg.memory.on`, `embed_model == "embeddinggemma"`, `topic_match == 0.80`, `gate_match == 0.85`.
  - `memory_on_parses`: `MEMORY=on` → `on`.
  - `memory_rejects_unknown`: `MEMORY=yes` → `ConfigError::Invalid { var: "MEMORY", .. }`.
  - `memory_threshold_range`: `MEMORY_GATE_MATCH=1.5` → `Invalid`.
  - `embeddings_need_url_and_auth`: only `GEMMA_URL` set → `!embeddings_enabled()`; both → `embeddings_enabled()`.
  - `gemma_auth_not_in_debug`: `format!("{:?}", cfg.memory)` does not contain the auth value.
- [ ] **Step 2: Run** `cargo test -p incisive-app config::` → FAIL.
- [ ] **Step 3: Implement** with the `TEMPORAL` match pattern; `MEMORY=on` with `TEMPORAL` off is allowed (log a warning in `main.rs` at startup, no error).
- [ ] **Step 4: Run** → PASS; full suite green.
- [ ] **Step 5: Commit** `feat(memory): MEMORY switch and settings (off by default)`

---

## Stage M1: pure extraction and matching

### Task 3: `memory::extract`

**Files:**
- Create: `src/memory/extract.rs`
- Test: `src/memory/extract.rs` `#[cfg(test)]`, using `tests/fixtures/temporal/{cholecystitis,hernia,achalasia}.json` via `build(&TemporalInput::from_json(..))` (pattern `src/temporal/tests.rs:1192`) and `tests/fixtures/temporal/full/*.json` for AnswerV3 bodies

**Interfaces:**
- Consumes: `crate::temporal::{TemporalTable, Era, StudyEntry, OutcomeResult, GroupResult, EraCell, Est}`; `ExpertBoardV1.topic`.
- Produces:
  ```rust
  pub struct Extracted {
      pub topic_label: String,                 // board topic, trimmed, clipped to 200 chars
      pub pairs: Vec<(String, String)>,        // normalised (intervention, comparator), sorted, deduped
      pub studies: Vec<LedgerStudy>,
      pub claims: Vec<LedgerClaim>,
      pub fts_extra: String,                   // outcome names + first at_a_glance line, space-joined
      pub embed_text: String,                  // topic_label + "\n" + pairs + "\n" + outcome names
  }
  pub struct LedgerStudy { pub pmid: String, pub pub_year: Option<i32>, pub treatment_start: Option<i32>,
                           pub treatment_end: Option<i32>, pub years_assumed: bool, pub era_id: Option<String> }
  pub struct LedgerClaim { pub outcome: String, pub intervention: String, pub comparator: String,
                           pub era_id: String, pub era_start: i32, pub era_end: i32,
                           pub estimate: Option<f64>, pub lo: Option<f64>, pub hi: Option<f64>,
                           pub effect_type: String, pub n_studies: i32, pub label: String }
  pub enum ExtractError { NotV3, NoAnswer, BadTable, BadBoard, NoTopic }
  impl ExtractError { pub fn code(&self) -> &'static str } // "not_v3","no_answer","bad_table","bad_board","no_topic"
  pub fn extract(body: &serde_json::Value, table_json: &serde_json::Value, board_json: &serde_json::Value)
      -> Result<Extracted, ExtractError>;
  pub fn normalise_arm(s: &str) -> String;  // lowercase, trim, collapse whitespace, strip "(…)"
  ```
- Rules the implementer cannot infer:
  - Studies: every `StudyEntry` with non-empty `pmid`; `years_assumed = entry.assumed`.
  - Claims: one per non-`empty` `EraCell` of each `GroupResult` of each `OutcomeResult`. `estimate`/`lo`/`hi` from `cell.pooled` (`Est.est/lo/hi`) if present, else `estimate = cell.estimate`, `lo`/`hi` None. `era_start`/`era_end` from the matching `Era`. `effect_type` = serde string of `group.family`; `label` = serde string of `group.label`; `n_studies` = distinct `study_id`s over `cell.row_ids`.
  - Pairs from `GroupResult.intervention/comparator` through `normalise_arm`; skip pairs where either side is empty.

- [ ] **Step 1: Failing tests:**
  - `extract_three_fixtures`: for each fixture, `studies.len()` equals the count of table studies with non-empty pmid; every claim's `era_id` exists in `table.eras`; `pairs` non-empty and sorted.
  - `claims_match_table`: for the hernia fixture, the claims' (outcome, era_id, estimate) set equals the one computed directly from `table.outcomes` cells with `pooled` (golden list written into the test from the current `build` output).
  - `normalise_arm_cases`: `"Robotic  (RAS)"` → `"robotic"`; `" Laparoscopic "` → `"laparoscopic"`.
  - `rejects_v2_body` (Review Focus 1): body with `"version": 2` → `Err(NotV3)`; table JSON `{}` → `Err(BadTable)`; board without topic → `Err(NoTopic)`.
- [ ] **Step 2: Run** `cargo test -p incisive-app memory::extract` → FAIL.
- [ ] **Step 3: Implement** `extract` (deserialize `TemporalTable` and `ExpertBoardV1` from the JSON values; check `body["version"] == 3` and `body["answer"]` is an object).
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(memory): pure extraction of topic, ledger and claims`

### Task 4: `memory::matcher`

**Files:**
- Create: `src/memory/matcher.rs`
- Test: in-file `#[cfg(test)]`

**Interfaces:**
- Produces:
  ```rust
  pub struct Candidate<'a> { pub pairs: &'a [(String, String)], pub embedding: Option<&'a [f32]>, pub fts_rank: f32 }
  pub struct TopicFeatures { pub topic_id: i64, pub pairs: Vec<(String, String)>, pub embedding: Option<Vec<f32>>, pub fts_rank: f32 }
  pub fn jaccard(a: &[(String, String)], b: &[(String, String)]) -> f32;  // pairs compared order-insensitively: (x,y)==(y,x)
  pub fn cosine(a: &[f32], b: &[f32]) -> f32;                              // 0.0 if lengths differ or a norm is 0
  pub fn score(c: &Candidate, t: &TopicFeatures) -> f32;
  pub fn best(c: &Candidate, topics: &[TopicFeatures]) -> Option<(i64, f32)>;  // highest score, ties → lowest topic_id
  ```
- Score formula (fixed here, thresholds tuned in Task 13). `fts_rank` arrives normalised to 0..1 by the store.
  - With both embeddings: `0.5*cosine + 0.3*jaccard + 0.2*fts_rank`.
  - Otherwise: `0.6*jaccard + 0.4*fts_rank`.

- [ ] **Step 1: Failing tests:** `jaccard_order_insensitive` (`[("robotic","laparoscopic")]` vs `[("laparoscopic","robotic")]` → 1.0); `jaccard_disjoint_zero`; `cosine_mismatched_len_zero`; `score_without_embeddings_uses_fallback_weights` (jaccard 1, fts 0.5 → 0.8); `best_breaks_ties_by_lowest_id`; `best_empty_none`.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(memory): topic matcher`

---

## Stage M2: storage, job, backfill

### Task 5: `memory::store`

**Files:**
- Create: `src/memory/store.rs`
- Test: `tests/memory_store.rs`

**Interfaces:**
- All functions take `&PgPool` (or `&mut PgConnection` inside a transaction) and return `sqlx::Result<_>`.
  ```rust
  pub fn vec_literal(v: &[f32]) -> String;   // "[0.1,0.2,...]"
  pub async fn resolve_topic(pool: &PgPool, topic_id: i64) -> sqlx::Result<i64>;  // follows merged_into to the end; cycle-safe (stop after 16 hops)
  pub async fn topic_candidates(pool: &PgPool, fts_query: &str, embedding: Option<&[f32]>, limit: i64)
      -> sqlx::Result<Vec<TopicFeatures>>;   // live topics only (merged_into IS NULL); fts_rank = ts_rank_cd normalised by max in result set
  pub async fn create_topic(tx: &mut PgConnection, label: &str, pairs: &[(String, String)]) -> sqlx::Result<i64>;
  pub struct NewRecord<'a> { pub topic_id: i64, pub source_run_id: Uuid, pub question: &'a str,
      pub answer: &'a Value, pub answer_version: i16, pub board: &'a Value, pub era_table: &'a Value,
      pub evidence_through: NaiveDate, pub topic_label: &'a str, pub fts_extra: &'a str }
  pub async fn insert_record(tx: &mut PgConnection, r: &NewRecord<'_>) -> sqlx::Result<(i64, i32)>; // (record_id, version = max+1 per topic)
  pub async fn upsert_ledger(tx: &mut PgConnection, topic_id: i64, record_id: i64, studies: &[LedgerStudy], claims: &[LedgerClaim]) -> sqlx::Result<()>;
  pub async fn set_record_embedding(pool: &PgPool, record_id: i64, v: &[f32], model: &str) -> sqlx::Result<()>;
  pub async fn set_topic_embedding(pool: &PgPool, topic_id: i64, v: &[f32], model: &str) -> sqlx::Result<()>;
  pub async fn enqueue_job(pool: &PgPool, run_id: Uuid) -> sqlx::Result<bool>;   // ON CONFLICT DO NOTHING
  pub async fn claim_jobs(pool: &PgPool, n: i64) -> sqlx::Result<Vec<Uuid>>;       // pending (or running with claimed_at older than 10 min), attempts < 3; FOR UPDATE SKIP LOCKED; sets running, claimed_at=now(), attempts+=1
  pub async fn finish_job(pool: &PgPool, run_id: Uuid, status: &'static str, code: Option<&'static str>, record_id: Option<i64>) -> sqlx::Result<()>;
  pub async fn records_missing_embedding(pool: &PgPool, n: i64) -> sqlx::Result<Vec<(i64, String)>>; // (id, embed text rebuilt from topic_label + fts_extra)
  ```
- Ledger rules: `ledger_studies` upsert on `(topic_id, pmid)`; existing row keeps `first_record_id`, updates `last_record_id`; `ledger_claims` plain insert keyed by record.

- [ ] **Step 1: Failing tests:**
  - `insert_versions_increment`: two records on one topic → versions 1 and 2.
  - `ledger_dedups_pmids`: two records sharing PMID "123" → one `ledger_studies` row, `first_record_id` = first, `last_record_id` = second.
  - `resolve_follows_chain` (Review Focus 3): A→B→C → `resolve_topic(A) == C`; a cycle A→B→A returns without hanging.
  - `candidates_exclude_merged`: a merged topic never appears in `topic_candidates`.
  - `claim_is_exclusive` (Review Focus 2): one pending job; two concurrent `claim_jobs(pool, 5)` via `tokio::join!` → total claimed ids = 1.
  - `vec_literal_round_trip`: write a 768-vector, read it back as text, same values to 1e-6.
- [ ] **Step 2: Run** `cargo test -p incisive-app --test memory_store` → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(memory): store for topics, records, ledger, jobs`

### Task 6: memory job, trigger and loop

**Files:**
- Create: `src/memory/job.rs`
- Modify: `src/runs/temporal.rs` (after `finish(...)` at ~:385 returns `Ok(true)` with `JobStatus::Done`, and `state.memory.on`: `memory::store::enqueue_job(&state.pool, run_id)`; errors ignored with a code-only log), `src/main.rs` (spawn `memory::job::memory_loop(state.clone())` next to `retention_loop` when `memory.on`)
- Test: `tests/memory_job.rs`

**Interfaces:**
- Consumes: Tasks 3–5; `TemporalView`-equivalent read of `run_temporal` (`table_json`, `board_json`, status); `crate::runs::store::load_question(pool, run_id)`; the run body from `run_answers`.
- Produces:
  ```rust
  pub async fn process(state: &AppState, run_id: Uuid) -> JobOutcome;
  pub enum JobOutcome { Done { record_id: i64, topic_id: i64, new_topic: bool }, Skipped(&'static str), Failed(&'static str) }
  pub async fn memory_loop(state: AppState);   // every 30 s: claim_jobs(…, 5) → process each → finish_job; then embedding sweep (Task 8)
  ```
- `process`: load run (mode must be Focused, else `Skipped("not_full")`), answer body, temporal row (must be `done`, else `Skipped("no_opinion")`) → `extract` (error → `Skipped(code)`) → `topic_candidates` + `matcher::best` (embedding: none here; Task 8 adds it) → if score ≥ `state.memory.topic_match`, `resolve_topic`, else `create_topic` → one transaction: `insert_record` + `upsert_ledger` → commit. `evidence_through` = run `created_at` date in IST. A foreign-key failure on a deleted run → `Failed("run_gone")`.

- [ ] **Step 1: Failing tests:**
  - `hernia_run_becomes_record`: seed a Focused run with `full/hernia.json` body and a `done` run_temporal row (table from `build`, board fixture with topic) → `process` → `Done { new_topic: true }`; record answer JSON byte-equal to the stored body.
  - `same_topic_second_run_joins`: second hernia run → `Done { new_topic: false }`, version 2.
  - `different_topic_new`: achalasia after hernia → new topic.
  - `quick_run_skipped`: Quick run → `Skipped("not_full")`.
  - `run_untouched` (Global Constraint): `runs` row and `run_answers.body` identical before and after `process`.
  - `run_deleted_mid_flight` (Review Focus 5): delete the run after claiming → `Failed("run_gone")` or the job cascades away; no `evidence_records` row created.
  - `trigger_only_when_on`: with `memory.on=false`, finishing a temporal job enqueues nothing; with `on=true`, exactly one job.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS; full suite green (existing temporal tests unchanged).
- [ ] **Step 5: Commit** `feat(memory): post-opinion memory job and loop`

### Task 7: backfill command

**Files:**
- Create: `src/bin/memory_backfill.rs`
- Test: `tests/memory_job.rs` (add)

**Interfaces:**
- Produces: `pub async fn backfill(pool: &PgPool) -> sqlx::Result<u64>` in `src/memory/job.rs`: enqueue every run with `mode='focused'` and `run_temporal.status='done'` that has no `memory_jobs` row; returns the count. The binary connects with `DATABASE_URL`, calls it, prints the count only.

- [ ] **Step 1: Failing test** `backfill_enqueues_once`: two eligible runs, one Quick, one Full without an opinion → returns 2; a second call returns 0.
- [ ] **Step 2–4:** run → FAIL, implement, run → PASS.
- [ ] **Step 5: Commit** `feat(memory): backfill existing Full runs`

---

## Stage M3: embeddings

### Task 8: `memory::embed` client, sweep and secret wiring

**Files:**
- Create: `src/memory/embed.rs`
- Modify: `src/memory/job.rs` (`process` embeds the new record and, if new, the topic, best-effort after commit; `memory_loop` sweeps `records_missing_embedding(pool, 20)` each tick), the deploy workflow in `.github/workflows/` (add `GEMMA_AUTH=gemma-auth:latest` to `--set-secrets`, and `GEMMA_URL`, `MEMORY` from repo variables), `infra/README.md` (owner commands below)
- Test: in-file tests with `wiremock`

**Interfaces:**
- Produces:
  ```rust
  pub struct Embedder { /* reqwest::Client, url, auth (SecretString), model */ }
  impl Embedder {
      pub fn from_settings(s: &MemorySettings) -> Option<Embedder>;  // None unless embeddings_enabled()
      pub async fn embed(&self, text: &str, timeout: Duration) -> Result<Vec<f32>, EmbedError>;
  }
  pub enum EmbedError { Timeout, Unauthorized, Http(u16), BadBody, WrongDim(usize) }
  impl EmbedError { pub fn code(&self) -> &'static str }
  ```
  - POST `{url}/api/embed`, basic auth, JSON `{"model": model, "input": text, "keep_alive": "30m"}`; accept only `embeddings[0]` of length 768.
  - `AppState` gains `embedder: Option<Arc<Embedder>>` (built in `main.rs`).
- Owner commands for `infra/README.md` (run in Cloud Shell, password pasted at the prompt, never on the command line):
  ```
  read -s -p "Gemma password: " P; printf 'incisive:%s' "$P" | gcloud secrets create gemma-auth --data-file=- --replication-policy=user-managed --locations=asia-south1 --project=incisive-prod-512902; unset P
  gcloud secrets add-iam-policy-binding gemma-auth --member=serviceAccount:incisive-run@incisive-prod-512902.iam.gserviceaccount.com --role=roles/secretmanager.secretAccessor --project=incisive-prod-512902
  ```

- [ ] **Step 1: Failing tests (wiremock):**
  - `embed_ok_768`: 200 with 768 floats → `Ok(v)`, `v.len()==768`; request carries a basic-auth header and model `embeddinggemma`.
  - `embed_401`: → `Err(Unauthorized)`.
  - `embed_timeout`: delayed response beyond the timeout → `Err(Timeout)`.
  - `embed_html_404` (Review Focus 4): 404 with an HTML body → `Err(Http(404))`; 200 with `{"error":"model not found"}` → `Err(BadBody)`; 200 with 384 floats → `Err(WrongDim(384))`.
  - `auth_not_in_debug`: `format!("{:?}", embedder)` has no password.
  - `record_embedded_after_job` (integration, `tests/memory_job.rs`): with a wiremock embedder, `process` leaves `embedding` non-NULL; with the mock returning 500, the record exists with NULL embedding and the next sweep fills it once the mock recovers.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement**; job and matcher now pass the record's embedding into `Candidate`. **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(memory): Gemma embeddings via owner VPS with fallback`

**Stage gate (M0–M3):** one PR; owner smoke: set `MEMORY=on` (with `TEMPORAL=on`), run `memory_backfill`, then check in the Supabase SQL editor that the three test runs became records with embeddings. Nothing is visible in the UI yet.

---

## Stage M4: owner Topics area (after temporal T4 is on `main`)

### Task 9: Topics domain functions and server fns

**Files:**
- Create: `src/memory/topics.rs`
- Modify: `src/app.rs` (server fns beside `get_activity` ~:379)
- Test: `tests/memory_topics.rs`

**Interfaces:**
- Every function takes `(&AppState, &User, …)` and starts with the owner check → `TopicsError::NotOwner`. Server fns map `NotOwner` → `StatusCode::FORBIDDEN`, as `get_activity` does.
  ```rust
  pub async fn list(state, user, search: Option<&str>) -> Result<Vec<TopicRow>, TopicsError>;  // live topics; search = FTS (+ embedding if available) ranked
  pub async fn detail(state, user, topic_id: i64) -> Result<TopicDetail, TopicsError>;         // versions, ledger studies by era, claims by era per version, verdict history, revisions
  pub async fn set_verdict(state, user, topic_id: i64, verdict: Verdict, note: &str, based_on: Option<i64>) -> Result<i64, TopicsError>;
  pub async fn add_revision(state, user, record_id: i64, part: RevisionPart, part_key: Option<&str>, owner_text: &str) -> Result<i64, TopicsError>; // copies claude_text from the record at call time
  pub async fn rename(state, user, topic_id: i64, label: &str) -> Result<(), TopicsError>;
  pub async fn merge(state, user, from: i64, into: i64) -> Result<(), TopicsError>;         // sets merged_into; moves nothing; logs payload
  pub async fn split(state, user, record_ids: &[i64], new_label: &str) -> Result<i64, TopicsError>; // new topic; records' topic_id moved; ledger rebuilt for both topics from their records' era tables
  pub async fn undo_last(state, user) -> Result<(), TopicsError>;                           // reverses the newest topic_changes row by its payload
  pub enum Verdict { Adopt, Selective, NotYet }
  pub enum RevisionPart { AnswerBottomLine, AnswerSection, Opinion, OpinionPart }
  ```
  - `TopicRow { id, label, versions: i32, latest_evidence: NaiveDate, verdict: Option<Verdict> }`.
  - Ledger rebuild after a split reuses `extract` over each record's stored `era_table` and is the only place ledgers are rewritten.

- [ ] **Step 1: Failing tests:** `non_owner_forbidden` (every function → `NotOwner` for a surgeon); `verdicts_append_only` (two verdicts → two rows, detail shows newest as current); `revision_copies_claude_text`; `merge_then_undo_restores`; `split_then_undo_restores` (record topic ids and ledger counts back to before); `rename_logged`.
- [ ] **Step 2–4:** run → FAIL, implement, run → PASS.
- [ ] **Step 5: Commit** `feat(memory): owner topics domain (verdicts, revisions, merge/split/undo)`

### Task 10: Topics pages

**Files:**
- Create: `src/app/topics_page.rs` (`TopicsPage`, `TopicPage`)
- Modify: `src/app.rs` routes (~:575): `<Route path=path!("topics") view=topics_page::TopicsPage />` and `<Route path=path!("topics/:id") view=topics_page::TopicPage />`; owner-only nav link beside Activity
- Test: server-render tests in the style of existing page tests; owner smoke

- [ ] **Step 1: Mockup** (Ivory/Figtree tokens, desktop + phone) of the list, the topic page (versions, ledger by era, verdict history and form, revision editor showing the Claude passage beside the owner text, merge/split/undo). **Owner approves before code.**
- [ ] **Step 2: Failing tests:** `topics_page_renders_rows` (SSR HTML contains each topic label and the verdict chip); `revision_view_labels_owner` (a record with a revision renders "Reviewed by the owner" and keeps the Claude text reachable); non-owner gets the 403 page.
- [ ] **Step 3: Implement** to the approved mockup; reuse `page::answer_page_view` and the temporal board view for opening a version.
- [ ] **Step 4: Run** tests, release hydrate build, phone-width check → PASS.
- [ ] **Step 5: Commit** `feat(memory): owner Topics pages`

---

## Stage M5: gate

### Task 11: `memory::gate` and the stored-answer view

**Files:**
- Create: `src/memory/gate.rs`
- Modify: `src/app.rs` (server fns `check_memory(question: String) -> Result<GateResult, ServerFnError>` and `get_stored(record_id: i64) -> Result<StoredView, ServerFnError>`)
- Test: `tests/memory_gate.rs`

**Interfaces:**
  ```rust
  pub enum GateResult { None, Strong { record_id: i64, topic_label: String, evidence_through: NaiveDate, score: f32 } }
  pub async fn check(state: &AppState, question: &str) -> GateResult;  // memory off → None; FTS candidates + query embedding (2 s timeout, failure ignored); matcher::best vs latest record of each live topic; Strong if score ≥ state.memory.gate_match
  pub struct StoredView { pub topic_label: String, pub evidence_through: NaiveDate, pub version: i32,
      pub answer: Value, pub board: Value, pub era_table: Value, pub owner_revisions: Vec<RevisionView> }  // no question, no run id, no user
  pub async fn stored(state: &AppState, user: &User, record_id: i64) -> Option<StoredView>; // any signed-in user (D6); record must be the latest version of a live topic or reachable by owner
  ```
  - The query FTS uses `websearch_to_tsquery('english', $1)`.

- [ ] **Step 1: Failing tests:** `gate_off_returns_none`; `gate_finds_paraphrase` (seed the hernia record; "Is robotic better than laparoscopic for groin hernia repair?" → Strong with a threshold set low in the test); `gate_ignores_unrelated` ("achalasia POEM vs Heller" → None against a hernia-only store); `gate_skips_merged_topic` (Review Focus 3); `stored_view_has_no_question` (serialised JSON does not contain the seeded question text or the run id); `surgeon_can_open_stored` (D6); `gate_survives_embedder_down` (wiremock 500 → still a result from FTS).
- [ ] **Step 2–4:** run → FAIL, implement, run → PASS.
- [ ] **Step 5: Commit** `feat(memory): gate and de-identified stored-answer view`

### Task 12: Ask-page gate card and stored page

**Files:**
- Modify: `src/app/ask_page.rs` (call `check_memory` on submit before `ask_question`; show the card on `Strong`), `src/app.rs` route `<Route path=path!("stored/:id") view=StoredPage />`
- Create: `src/app/stored_page.rs`
- Test: SSR tests; owner smoke

- [ ] **Step 1: Mockup** of the card ("An answer on this topic already exists (evidence through <Mon YYYY>)." with **Open it**, **Update with newer evidence**, **Ask anyway**) and the stored page (topic label header, answer, Expert Opinion, owner-reviewed labels). **Owner approves before code.**
- [ ] **Step 2: Failing tests:** `card_copy_exact` (renders the sentence with the month-year date); `update_starts_full_run` (Update dispatches `ask_question` with `RunMode::Focused` and the user's question); `ask_anyway_runs_normally`; `stored_page_shows_topic_not_question`.
- [ ] **Step 3: Implement** to the mockup. The gate check must not block submit for more than its 2 s budget plus DB time; on any error, proceed to `ask_question`.
- [ ] **Step 4: Run** tests, release hydrate build → PASS.
- [ ] **Step 5: Commit** `feat(memory): Ask-page gate and stored answer page`

---

## Stage M6: calibration

### Task 13: Gate and topic thresholds

**Files:**
- Create: `docs/goldset/memory-paraphrases.toml` (for each gold-set question: two paraphrases written by the builder, **reviewed by the owner**, plus 10 unrelated surgical questions), `src/bin/memory_calibrate.rs`
- Modify: `src/memory/settings.rs` defaults, `docs/build-log/memory/` report

**Interfaces:**
- `memory_calibrate` reads the paraphrase file and the live store (read-only, `DATABASE_URL`, embedder from env) and prints, for thresholds 0.50–0.95 in steps of 0.05: paraphrase hit rate, unrelated false-positive rate. No question text is printed, only ids and rates.
- Choose `gate_match` = the lowest threshold with **0 false positives** on the unrelated set and the highest paraphrase hit rate; `topic_match` the same, from record-to-record matching. Record both, and the curve, in the build log.

- [ ] **Step 1:** owner runs Full runs for enough gold-set topics to seed the store (owner's choice; cost within the daily cap).
- [ ] **Step 2:** run `memory_calibrate`; write the table to `docs/build-log/memory/calibration.md`.
- [ ] **Step 3:** set the defaults in `MemorySettings::default()`, update the `memory_defaults_off` test values, commit `chore(memory): calibrated thresholds`.
- [ ] **Step 4:** live smoke with `MEMORY=on`: paraphrase → card, Open → stored page, Update → a normal Full run → becomes version 2 of the topic; owner verdict and revision visible.
- [ ] **Step 5:** owner "merge it"; update the handover (`plans/incisive-handover.md`) and the build log.
