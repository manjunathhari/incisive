# Incisive Evidence Chat: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Chat option on Full runs with a finished Expert Opinion, where Gemma 3 4B answers only from that run's stored evidence and code checks every reply.

**Architecture:** A new `src/chat/` module beside `src/memory/`. Pure parts (passages, comparison, checks, fixed lines, prompt) have no I/O. A separate `chat_loop` prepares passages from existing evidence records. Each question becomes a `pending` row in `chat_turns`; a background task streams Gemma into that row and the page polls it. Everything sits behind `CHAT=off|on`, migration 0014 is additive, and no run depends on it.

**Tech Stack:** Rust 2024, Leptos 0.8 + Axum, sqlx 0.8 (plain `sqlx::query`), Postgres 16 + pgvector, reqwest 0.13 (add `stream`), wiremock 0.6, Ollama `gemma3:4b` + `embeddinggemma` (768-dim).

**Spec:** `docs/superpowers/specs/2026-11-02-incisive-evidence-chat-design.md` (decisions C1–C10)

## Where this plan departs from the spec

Reading the code changed these. The owner confirms them when reviewing this plan; Task 1 then records them in the spec.

| # | Spec says | Plan does | Why |
|---|---|---|---|
| P1 | `quote` passages from extractor and skeptic | **Skeptic quotes only (3–7 per run).** The structured numbers in each outcome row stay in the `outcome` passages. | Extractor quotes (41–69 per run) are checked, then discarded; only their count is stored (`EvidenceV3.verified_quotes`). Keeping them is an engine change, so it waits for Part B under the freeze. |
| P2 | Reply streams to the browser | Gemma streams to the server, which writes the row at most every 500 ms; the page polls every 700 ms. | Cloud Run has up to 3 instances, so a follow-up request can land on another instance. There is no SSE in the app today. The reply also survives a locked phone. |
| P3 | Single `comparison`, `ref_n` | `comparisons TEXT[]`, `ref_ns INT[]` | Many passages cover several arms or several papers (e.g. "POEM vs LHM vs PD"). |
| P4 | Passage kinds as listed in §4 | Also `guideline`, `clinical_practice`, `notable` and the completeness review's additions | Spec §1 says "its answer", and "who may benefit" and counselling are what patients ask about. |
| P5 | (silent) | **Every chat question goes through `deid::scan`.** A hit is blocked before anything is stored or sent. | Personal questions are the expected use, and Ask already has this rule. |
| P6 | Belief passages pass number checks | The "Common belief" half must contain **no numbers**. A claim with numbers gets no belief passage. | A corrected belief's own numbers are wrong by definition, and Gemma would otherwise repeat them. |
| P7 | Evaluation is a bin | Owner-only action in the app; results are ordinary turns on a flagged conversation. | Bins can't run in production: `memory_backfill` had to be replaced by SQL, and Cloud Shell has no cargo. |
| P8 | Prep inside the memory job | Separate `chat_loop` that finds records with no `chat_prep` row | Covers backfill and retries with one path, leaves the memory job alone, and a slow Gemma call can't hold up memory jobs. |
| P9 | `CHAT` needs `MEMORY=on` | `CHAT=on` with `MEMORY=off` means chat is off, with a warning, not a boot failure | `MEMORY=off` must stay a safe rollback on its own. |
| P10 | §10 lists four variables | Adds `CHAT_MIN_SCORE` (default 0.45, placeholder until Task 13) | The threshold must be tunable without a release. |

## Global Constraints

- Paths are relative to `crates/incisive-app/` unless they start with `migrations/`, `docs/`, `.github/`, `infra/` or `.10x/`.
- **Engine frozen.** The only permitted diff under `src/agent/` is making `traced` public in `temporal_writer.rs`. Check `git diff main -- crates/incisive-app/src/agent/` before every PR and revert anything else.
- No chat step may fail, delay or alter a run, an Expert Opinion or a memory job. Gemma being down never affects them.
- Never log question, reply, passage or prior text. Errors are fixed `&'static str` codes. The Gemma password stays in `GEMMA_AUTH`.
- `CHAT`: `off` (default) or `on`; anything else is `ConfigError::Invalid`. `CHAT_MODEL` default `gemma3:4b`. `CHAT_DAILY_LIMIT` default 30 per user. `CHAT_PRIOR`: `off` or `on`, default on when `CHAT` is on.
- Gemma chat: `POST {GEMMA_URL}/api/chat`, `num_predict` 300, `keep_alive` "30m", 60 s limit, last 3 turns as context. Retrieval keeps the top 3 passages.
- **C8 by type:** a grounded request can only be built from `GroundedPassages`, whose constructor returns `None` for an empty list.
- Exact text (in `chat/lines.rs` only):
  - `Not covered in this evidence.`
  - `Your own surgeon can tell you what this means for you.`
  - `If you have these symptoms now, contact your surgeon or hospital straight away.`
  - `Chat is unavailable right now.`
  - `Gemma is busy, try again in a minute.`
  - `Answers come only from this run's evidence. Educational, not medical advice.`
- Flag, never delete: checks mark a reply and never remove text.
- A user reaches only their own runs and turns; anyone else's id is "not found", as in `runs/api.rs`. Owner-only functions check `user.role != Role::Owner`.
- House process per stage: builder subagent, independent reviewer, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p incisive-app --lib --no-default-features --features hydrate -- -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo build -p incisive-app --lib --no-default-features --features hydrate --release`. PR, CI green, merge only on the owner's "merge it".
- UI tasks start with a mockup the owner approves on his phone. Change only what he names.
- Stage files by explicit path; never `git add -A`. Each stage updates `plans/incisive-chat-build-log.md` in the Project and the handover.
- **PRs:** A = Tasks 1–9 (CHAT off, nothing visible). B = Task 10. C = Task 11. D = Task 12. E = Task 13. Cherry-pick the `memory-live` build-log commit into PR A.

## Review Focus

1. **Warning symptoms on a path that gives no answer** (not covered, identifier blocked, daily limit, busy, Gemma down): the safety line still appears. Test in Task 8.
2. **An identifier in a chat question** ("Mr Sharma, 12 March, what about his hernia?"): blocked with the Ask page's hint; nothing is stored or sent to Gemma. Test in Task 8.
3. **The stored answer's own citation tokens** (`[3]`, `[pmid as n]`) reaching Gemma, or `[1]` in a reply being read as a number: tokens are stripped when passages are built and ignored when numbers are checked. Tests in Tasks 3 and 6.
4. **The phone locks, the tab closes or the instance dies mid-reply:** the reply finishes and is stored without the browser; a dead instance's `pending` turn is resolved within 90 s, so the user isn't locked out. Test in Task 8.
5. **A short follow-up** ("how sure is that?") must not fall to "Not covered" for lack of words: retrieval includes the previous question when the new one is short. Test in Task 5.

---

## PR A · Stage CH0: schema, switches, hardening

### Task 1: Migration 0014, chat settings, deploy variables

**Files:**
- Create: `migrations/0014_chat.sql`, `src/chat/mod.rs`, `src/chat/settings.rs`, `tests/chat_schema.rs`
- Modify: `src/lib.rs`, `src/config.rs`, `src/server.rs` (`AppState.chat`), `Cargo.toml` (reqwest `stream`), `.github/workflows/deploy.yml`, `infra/README.md`, `docs/superpowers/specs/2026-11-02-incisive-evidence-chat-design.md`

**Interfaces:**
- Produces: `ChatSettings { on: bool, model: String, daily_limit: i64, prior: bool, min_score: f32 }`; `AppState.chat: ChatSettings`.
- Tables (RLS on, no policies, as 0013). `chat_passages`, `chat_prep` and `topic_qas` are permanent; turns follow the run:
  - `chat_passages`: `id BIGSERIAL PK`, `record_id → evidence_records`, `kind`, `seq INT`, `ref_ns INT[] NOT NULL DEFAULT '{}'`, `pmid`, `comparisons TEXT[] NOT NULL DEFAULT '{}'`, `text`, `settling_passage_id → chat_passages NULL`, `fts` generated, `embedding vector(768)`, `embed_model`, `embed_attempts INT NOT NULL DEFAULT 0`, `created_at`; `UNIQUE (record_id, kind, seq)`; gin on `fts`, btree on `record_id`. No hnsw: retrieval scans one record's rows.
  - `chat_prep`: `record_id PK → evidence_records`, `status CHECK IN ('ok','failed')`, `passages INT`, `error_kind`, `created_at`.
  - `chat_conversations`: `id`, `run_id → runs ON DELETE CASCADE`, `user_id → users`, `mode CHECK IN ('clinical','plain')`, `is_eval BOOL NOT NULL DEFAULT FALSE`, `created_at`.
  - `chat_turns`: `id`, `conversation_id ON DELETE CASCADE`, `user_id`, `seq`, `question`, `reply NOT NULL DEFAULT ''`, `mode`, `status CHECK IN ('pending','done','partial','not_covered','unavailable')`, `passage_ids BIGINT[]`, `checks JSONB`, `safety_line BOOL`, `personal BOOL`, `feedback SMALLINT NULL CHECK IN (-1, 1)`, `suggested BOOL NOT NULL DEFAULT FALSE`, `latency_ms`, `created_at`, `updated_at`; `UNIQUE (conversation_id, seq)`; **partial unique index on `(user_id) WHERE status = 'pending'`** (one reply in flight, across instances).
  - `topic_qas`, `chat_priors`: as spec §9, plus `chat_priors UNIQUE (run_id)` and `status CHECK IN ('writing','written','reconciling','reconciled','failed')`.
  - `ALTER TABLE incisive.evidence_records ADD COLUMN embed_attempts INT NOT NULL DEFAULT 0`.

- [ ] **Step 1: Write failing tests.** `chat_schema.rs`: `migration_applies_and_tables_exist`; `second_pending_turn_for_user_is_refused` (23505); `run_delete_cascades_conversations_and_turns_but_keeps_passages`; `run_delete_nulls_prior_and_qa_links`. `config.rs` tests: `chat_defaults_off`; `chat_invalid_value_is_config_error`; `chat_on_with_memory_off_is_off` (P9); `chat_prior_defaults_on_when_chat_on`; `chat_min_score_out_of_range_is_config_error`.
- [ ] **Step 2: Run them; expect failure** (`cargo test -p incisive-app --test chat_schema`, and `--lib config`). Local DB: `pg_ctlcluster 16 main start`, `postgres://t:t@localhost/incisive_test`.
- [ ] **Step 3: Implement the migration and settings.** Deploy: mirror `MEMORY` in `deploy.yml` (both env blocks, the validation block, the `optional` list) for `CHAT`, `CHAT_MODEL`, `CHAT_DAILY_LIMIT`, `CHAT_PRIOR`, `CHAT_MIN_SCORE`. Add an "Evidence chat" section to `infra/README.md`. In the spec: set Status to approved, correct the §6 heading (reconciled claims are for everyone), add the P1–P10 table.
- [ ] **Step 4: Run; expect PASS.**
- [ ] **Step 5: Commit** `feat(chat): migration 0014, CHAT switches and deploy variables`.

### Task 2: Gemma hardening parked from PR A

**Files:** Modify `src/config.rs`, `src/memory/job.rs`, `src/memory/store.rs`; Test `tests/memory_job.rs`, `tests/memory_store.rs`

**Interfaces:** Produces `store::note_embed_attempt(pool, record_id)`; private `SweepLock` guard in `job.rs`.

- [ ] **Step 1: Write failing tests.**
  - `gemma_url_rejects_backslash_bypass`: `http://evil.example\@localhost/` is refused; `https://x.example`, `http://localhost:11434`, `http://127.0.0.1` allowed; `http://localhost.evil.example` and `ftp://x` refused.
  - `sweep_lock_released_when_guard_dropped_unreleased`: drop the guard mid-way, then within 2 s `pg_try_advisory_lock` succeeds from another connection.
  - `record_stops_being_swept_after_five_failed_attempts`: bad-body responses → `records_missing_embedding` no longer returns it. A server-unavailable failure does not count.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement.** `gemma_url_allowed` uses `url::Url::parse`: `https` with a host, or `http` with host exactly `localhost`/`127.0.0.1`. `SweepLock` owns the connection; on drop without a clean unlock it calls `conn.detach()`, so a connection holding the lock never returns to the pool. `records_missing_embedding` adds `AND embed_attempts < 5`.
- [ ] **Step 4: Run; expect PASS,** with `tests/memory_job.rs` and `tests/memory_store.rs` still green.
- [ ] **Step 5: Commit** `fix(memory): real URL parsing, sweep-lock guard, embedding attempt cap`.

## PR A · Stage CH1: passages

### Task 3: Passage and comparison builders (pure)

**Files:** Create `src/chat/passages.rs`, `src/chat/comparison.rs`; Modify `src/memory/extract.rs` (expose `parse_board(&Value) -> Result<ExpertBoardV1, ExtractError>`); Test inline, using `tests/fixtures/temporal/full/{achalasia,hernia,cholecystitis}.json`.

**Interfaces:**
- Produces:
  - `PassageKind` (`bottom_line`, `outcome`, `evidence`, `conflict`, `applicability`, `what_would_change`, `guideline`, `clinical_practice`, `notable`, `opinion`, `abstract`, `quote`, `reference`, `owner_qa`, `belief`).
  - `NewPassage { kind, seq: i32, ref_ns: Vec<i32>, pmid: Option<String>, comparisons: Vec<String>, text: String }`.
  - `passages::build(body: &Value, board: &Value, pairs: &[(String, String)], abstracts: &[(String, String)]) -> Result<Vec<NewPassage>, BuildError>`.
  - `passages::starters(body: &Value) -> Vec<String>`: three questions from the first outcomes.
  - `comparison::key(a: &str, b: &str) -> String`: order-independent.
  - `comparison::tags(text: &str, pairs: &[(String, String)]) -> Vec<String>`.
  - `comparison::asked(question: &str, pairs: &[(String, String)]) -> Option<String>`.
  - `comparison::arms_named(text: &str, pairs: &[(String, String)]) -> Vec<String>`.

- [ ] **Step 1: Write failing tests.**
  - `counts_by_kind_match_fixture`: for each fixture, `outcome` = outcomes + `completeness.added_outcomes`, `evidence` = studies, `reference` = references, `conflict` = conflicts + `added_conflicts`, `quote` = skeptic entries with a quote.
  - `outcome_additions_reach_their_outcome_passage`.
  - `citation_tokens_are_stripped`: no passage text matches `\[\d+\]` or `[pmid as`, and `ref_ns` holds what they cited.
  - `not_traced_marks_are_kept_in_words`: a number marked untraced stays marked in the passage.
  - `abstract_splits_at_sentences_to_120_words`; `missing_abstract_gives_no_abstract_passage`.
  - `retracted_reference_is_labelled_and_its_abstract_left_out`.
  - `opinion_passages_cover_verdict_why_glance_eras_decisions`.
  - `multi_arm_study_gets_both_tags`: "POEM vs LHM vs PD" tags both POEM–LHM and POEM–PD in achalasia.
  - `untaggable_text_has_no_tags`.
  - `asked_finds_arms_not_in_the_record`: on cholecystitis, "If I have robotic instead of open…" gives `key("robotic","open")`, though the record has only robotic-vs-laparoscopic.
  - `asked_is_none_with_one_arm`; `keyhole_means_laparoscopic`; `key_ignores_order`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement.** Parse `AnswerV3` from the body; `BuildError` holds fixed codes. Arm matching uses `normalise_arm` plus a small fixed alias list: robotic (`robot`, `robot-assisted`, `r-tapp`, `rtapp`, `rtar`, `da vinci`), laparoscopic (`lap`, `keyhole`, `l-tapp`, `ltapp`, `tep`), open. A row's tags come from its own arms; an outcome row takes the union of its `pmids`' `Study.comparison` tags. Board passages take the record's pair when there is exactly one, else none.
- [ ] **Step 4: Run; expect PASS.**
- [ ] **Step 5: Commit** `feat(chat): passage and comparison builders`.

### Task 4: `chat_loop`: prep, vectors, stale turns

**Files:** Create `src/chat/store.rs`, `src/chat/job.rs`; Modify `src/main.rs` (spawn only when `state.chat.on`); Test `tests/chat_prep.rs`

**Interfaces:**
- Consumes: `passages::build`; `extract::normalise_arm`; `store::vec_literal`; `Embedder`.
- Produces: `store::records_needing_prep(pool, n) -> Vec<RecordForPrep>`; `store::prep_record(pool, record, abstracts, passages)` (one transaction); `store::read_abstracts(pool, pmids)`; `store::resolve_stale_turns(pool)`; `store::pending_turns(pool) -> i64`; `job::chat_loop(state)`; `job::tick(state)`; `job::sweep_passage_vectors(state) -> usize`.

- [ ] **Step 1: Write failing tests.**
  - `tick_prepares_existing_records_and_is_idempotent`; `bad_record_gets_failed_prep_and_others_still_run`.
  - `abstract_older_than_30_days_is_still_copied`: `lit::cache::get_many` hides anything over 30 days, and cached papers are pruned at 90, so this reads `paper_cache` directly.
  - `passages_survive_paper_cache_prune`.
  - `sweep_fills_vectors_and_gives_up_after_five_bad_attempts`.
  - `sweep_stops_when_server_unavailable`; `sweep_yields_when_a_turn_is_pending`: no embed calls while a user's reply is running.
  - `stale_pending_turn_becomes_partial_after_90s`: with text → `partial`; empty → `unavailable`.
  - `two_ticks_at_once_give_one_set_of_passages`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement.** Every 30 s: resolve stale turns; prep up to 5 records with no `chat_prep` row; under its own advisory lock (key "INCCHT01", using the Task 2 guard) sweep vectors with the 5 s embed timeout and a 25 s budget. The loop survives a panic as `memory_loop` does.
- [ ] **Step 4: Run; expect PASS.**
- [ ] **Step 5: Commit** `feat(chat): passage prep and vector sweep loop`.

## PR A · Stage CH2: answering

### Task 5: Retrieval

**Files:** Create `src/chat/retrieve.rs`; Test inline (pure) and `tests/chat_retrieve.rs`

**Interfaces:**
- Consumes: `comparison::asked`; `store::fts_words`.
- Produces: `retrieve::score(cos: Option<f32>, word_share: f32) -> f32`; `retrieve::query_text(question: &str, previous: Option<&str>) -> String`; `retrieve::top(pool, record_id, topic_id, asked: Option<&str>, query: &str, vector: Option<&[f32]>, min_score: f32) -> sqlx::Result<Vec<Hit>>`; `Hit { passage_id: Option<i64>, qa_id: Option<i64>, kind, text, ref_ns, score }`.

- [ ] **Step 1: Write failing tests.**
  - `score_blends_meaning_and_words`: with a vector `0.7*cos + 0.3*share`; without one, `share`.
  - `short_follow_up_includes_previous_question`: under 6 content words joins the previous question; 6 or more does not.
  - `other_comparison_excluded`: robotic-vs-laparoscopic tags are out when robotic-vs-open was asked.
  - `untagged_passages_stay`; `passage_with_both_tags_stays`.
  - `below_threshold_returns_empty`; `keeps_top_three`.
  - `null_vector_passage_can_match_on_words`.
  - `approved_qa_for_topic_is_included_and_other_statuses_are_not`.
  - `qa_of_merged_topic_is_found`: A merged into B; a run under A finds B's Q&As.
  - `other_records_passages_are_never_returned`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement** with plain SQL over one record's rows plus the topic's approved Q&As (recursive CTE over `merged_into`), scoring in Rust. Word share uses `fts_words`, matching any word, not all: PR A's final review found AND-style matching missed a run's own topic.
- [ ] **Step 4: Run; expect PASS.**
- [ ] **Step 5: Commit** `feat(chat): hybrid retrieval with comparison filter`.

### Task 6: Reply checks and fixed lines (pure)

**Files:** Create `src/chat/checks.rs`, `src/chat/lines.rs`; Modify `src/agent/temporal_writer.rs` (`fn traced` → `pub fn traced`, nothing else); Test inline

**Interfaces:**
- Produces: `Checks { not_traced: Vec<String>, unknown_citations: Vec<u32>, outside_references: Vec<String>, comparison_mismatch: Vec<String>, you_should: bool, uncited: bool }` (`Serialize`, `Deserialize`, `is_clean()`); `checks::run(reply: &str, passages: &[&str], asked: Option<&str>, pairs: &[(String, String)]) -> Checks`; `lines::warning_symptoms(q) -> bool`; `lines::personal(q) -> bool`; the exact strings from Global Constraints.

- [ ] **Step 1: Write failing tests.**
  - `number_in_cited_passage_passes`; `number_in_uncited_passage_is_not_traced`.
  - `citation_markers_are_not_numbers`: `[1]`, `[3]`.
  - `rounded_number_traces`: 4.2 against 4.17.
  - `small_counts_are_not_flagged`: "two trials", "3 studies".
  - `range_is_checked`: "7–10".
  - `unknown_citation_flagged`: `[4]` with 3 passages.
  - `author_year_flagged`: "Lee et al. 2021", "J Robot Surg 2019;33:12".
  - `other_arm_flagged`: reply mentions laparoscopic when robotic-vs-open was asked.
  - `no_asked_comparison_no_arm_flag`; `you_should_flagged_any_case`; `reply_without_citation_flagged_uncited`.
  - `regression_invented_references_are_flagged`: the 2 Nov no-passage reply, as a fixture string, trips `outside_references` and `not_traced`.
  - `warning_words_detected`: fever, bleeding, severe pain, increasing pain, chest pain, breathlessness, vomiting, wound redness, pus, unable to pass urine. `pain_alone_is_not_warning`.
  - `personal_detected`: "should I have", "will I go home sooner", "my mother's hernia". `general_question_is_not_personal`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement.** Numbers via `text_numbers` and `traced`, with the same small-count rule as the temporal check in a local `const SMALL_INT: f64 = 12.0` (no bare whole number up to 12 is flagged), over the union of cited passages for the whole reply (not sentence by sentence: Gemma's sentence boundaries are too loose for that).
- [ ] **Step 4: Run; expect PASS.** Confirm `git diff main -- crates/incisive-app/src/agent/` shows one word.
- [ ] **Step 5: Commit** `feat(chat): reply checks and fixed lines`.

### Task 7: Prompt and Gemma streaming client

**Files:** Create `src/chat/prompt.rs`, `src/chat/gemma.rs`; Modify `src/memory/embed.rs` (share `parse_auth`), `src/server.rs` (`AppState.gemma: Option<Arc<GemmaChat>>`), `src/main.rs`; Test inline with wiremock

**Interfaces:**
- Produces:
  - `prompt::GroundedPassages::new(Vec<Hit>) -> Option<GroundedPassages>`.
  - `prompt::grounded(p: &GroundedPassages, question: &str, history: &[(String, String)], mode: ChatMode) -> GemmaRequest`.
  - `GemmaChat::from_settings(memory: &MemorySettings, chat: &ChatSettings) -> Option<GemmaChat>`.
  - `GemmaChat::stream(&self, req: &GemmaRequest, limit: Duration, on_text: impl FnMut(&str)) -> Result<StreamEnd, GemmaError>`, with `StreamEnd::{Complete, CutAtLimit}`.
  - `GemmaChat::json(&self, req: &GemmaRequest, limit: Duration) -> Result<Value, GemmaError>` (used in Task 12).
  - `GemmaError` with `code()` and `is_server_unavailable()`, shaped like `EmbedError`.

- [ ] **Step 1: Write failing tests.**
  - `empty_passages_cannot_build_a_request`.
  - `system_rules_are_present`: answer only from the numbered passages; cite like [1]; stay within the comparison asked; exact "Not covered in this evidence"; never "you should"; no author/year/journal references. `plain_mode_adds_plain_rules`.
  - `passages_numbered_from_one`; `history_keeps_last_three_turns`.
  - `body_sets_num_predict_and_keep_alive`: 300, "30m", `stream: true`.
  - `stream_joins_chunks`, with a JSON line split across two HTTP chunks; `error_line_in_stream_is_an_error`.
  - `limit_keeps_partial_text`; `html_404_and_401_map_to_codes`; `redirect_is_not_followed`.
  - `auth_not_in_debug`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement** with `reqwest` `bytes_stream`, buffering to newlines.
- [ ] **Step 4: Run; expect PASS.**
- [ ] **Step 5: Commit** `feat(chat): grounded prompt and Gemma streaming client`.

## PR A · Stage CH3: chat API

### Task 8: Ask flow

**Files:** Create `src/chat/service.rs`; Modify `src/chat/store.rs`, `src/audit.rs` only if a new event is needed (reuse `Event::DeidBlocked`); Test `tests/chat_service.rs` (fake Gemma via wiremock)

**Interfaces:**
- Consumes: Tasks 3–7; `deid::scan`; `runs::store`.
- Produces:
  - `service::ChatState` (`Unavailable`, `NotOffered`, `GettingReady`, `Ready`).
  - `service::state_for(state, user, run_id) -> Result<ChatView, ChatError>`.
  - `service::ask(state, user, run_id, question, mode) -> Result<Asked, ChatError>`.
  - `Asked { turn_id: i64, safety_line: bool }`.
  - `ChatError` variants `NotFound`, `NotOffered`, `Empty`, `TooLong`, `Identifiers { message, safety_line }`, `InFlight`, `DailyLimit { safety_line }`, `Busy { safety_line }`, `Unavailable { safety_line }`, `Failed`.
  - `service::turn_view(state, user, turn_id) -> Result<TurnView, ChatError>`.

Order in `ask`: length (1–1000 chars) → safety and personal detection → `deid::scan` → run ownership and readiness → daily limit (today IST, by `user_id`) → busy (`pending_turns` ≥ 3) → insert `pending` (23505 → `InFlight`) → spawn → return. In the task: embed the query (3 s; a failure means words only) → retrieve → empty: `not_covered` with no Gemma call → stream, writing at most every 500 ms → checks → finish. Reply text "Not covered in this evidence" from Gemma is also stored as `not_covered`. The task catches panics and finishes the turn `unavailable`.

- [ ] **Step 1: Write failing tests.**
  - `no_passage_above_threshold_never_calls_gemma` (`expect(0)`). This is the **C8 regression**, using the question `If I have robotic TAR instead of open, will I go home sooner, and how sure is that?` on the achalasia fixture's record: reply is the exact not-covered line.
  - `normal_question_stores_reply_passages_checks_timing_mode`.
  - `safety_line_on_every_path`: not covered, identifiers, daily limit, busy, unavailable, normal.
  - `identifier_blocks_before_store_and_gemma`: no turn row, 0 calls, audit shows the kind only.
  - `reply_finishes_after_caller_goes_away`: drop the polling side; the turn reaches `done`.
  - `second_ask_while_pending_is_in_flight`; `dead_instance_turn_unblocks_user`.
  - `limit_counts_per_user_today_ist`; `fourth_pending_is_busy`.
  - `gemma_down_gives_unavailable_and_answer_unaffected`; `cut_at_60s_is_partial_with_text`.
  - `quick_run_and_run_without_opinion_not_offered`; `getting_ready_until_prep_ok`; `failed_prep_is_unavailable`.
  - `someone_elses_run_is_not_found`, including the owner's own attempt: the owner reads others' chats only in `/chats`.
  - `question_and_reply_never_logged`, using the `tests/library_logging.rs` capture.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run; expect PASS.**
- [ ] **Step 5: Commit** `feat(chat): ask flow with pending turns`.

### Task 9: Routes, feedback, suggest, retention

**Files:** Create `src/chat/api.rs`; Modify `src/server.rs` (routes under `same_origin_check`), `src/main.rs` (`retention_loop`), `src/chat/store.rs`; Test `tests/chat_api.rs`

**Interfaces:**
- Produces:
  - `GET /api/runs/{id}/chat`: state, starters, conversation, turns.
  - `POST /api/runs/{id}/chat`: `{question, mode}`.
  - `GET /api/chat/turns/{id}`: reply so far, status, checks, fixed lines.
  - `GET /api/chat/passages/{id}`: passage and reference, for citation taps; scoped to the user's own turns.
  - `POST /api/chat/turns/{id}/feedback`: `{value: 1|-1}`.
  - `POST /api/chat/turns/{id}/suggest`.
  - `store::retention_pass(pool)`.

- [ ] **Step 1: Write failing tests.**
  - `routes_404_when_chat_off`: `CHAT=off` leaves the API behaviour unchanged.
  - `cross_site_post_refused`; `other_users_turn_is_not_found_on_every_route`.
  - `passage_route_refuses_passage_not_in_users_turns`.
  - `suggest_is_idempotent_and_only_for_done_turns`; `suggest_files_under_resolved_topic`.
  - `body_limit_applies`.
  - `retention_removes_unapproved_qas_with_no_turn_and_priors_with_no_run_or_record_but_keeps_approved_and_linked`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement,** mirroring `runs/api.rs` errors.
- [ ] **Step 4: Run all house checks; expect PASS** (about 912 plus the new tests).
- [ ] **Step 5: Commit** `feat(chat): API, feedback, suggest, retention`.
- [ ] **Step 6: Whole-branch review on the most capable model, then fixes.** Open PR A; wait for "merge it". Live check with `CHAT=off`: nothing visible, Deploy green.

---

## PR B · Stage CH4: run-page Chat

### Task 10: Chat window (mockup first)

**Files:** Create `src/chat_page.rs` (pure rules), `src/app/chat.rs`; Modify `src/app/expert.rs` (third option), `src/app.rs` (server functions), `Cargo.toml` only if needed; Test inline and `tests/chat_ui.rs`

**Interfaces:** Consumes the Task 9 routes through `#[server(prefix = "/api/fn", ...)]` functions. Polling follows the existing `set_interval_with_handle` pattern in `app.rs`.

- [ ] **Step 1: Mockup artifact** on real output of the owner's bilateral TAR run, in phone and desktop widths, quiet ivory + Figtree: the three-way option and its states (getting ready, ready, unavailable), Clinical / Plain switch, starters, flag marks, fixed lines in their own style, 👍 / 👎, *Suggest for library*, footer. **Stop for owner approval.**
- [ ] **Step 2: Write failing tests.**
  - `option_absent_on_quick_and_no_opinion`.
  - `answer_and_expert_views_unchanged`: `tests/temporal_ui.rs` and the Part A fingerprints still pass byte-identical with `CHAT=off`.
  - `citations_render_only_for_given_passages`; `flag_labels_are_plain_words`.
  - `form_post_works_without_js`: the POST returns a page showing the pending turn, which reloads until it is finished, as the run page does before hydration.
- [ ] **Step 3: Run; expect failure.**
- [ ] **Step 4: Implement to the approved mockup.** Poll every 700 ms while `pending`; stop when the page is hidden and resume when visible, showing the stored result.
- [ ] **Step 5: Run all house checks, including the release hydrate build; expect PASS.** Nested `view!` trees may need `.into_any()`.
- [ ] **Step 6: Commit, review, PR B,** wait for "merge it".

## PR C · Stage CH5: owner view

### Task 11: `/chats` (mockup first)

**Files:** Create `src/chat/owner.rs`, `src/app/chats_page.rs`; Modify `src/app.rs` (route; link in the header for the owner only), `src/chat/store.rs`; Test `tests/chat_owner.rs`

**Interfaces:** Produces `owner::conversations`, `owner::conversation`, `owner::suggestions`, `owner::flagged`, `owner::approve(state, user, qa_id, question: Option<String>, text: Option<String>)`, `owner::reject`, each returning `NotOwner` for anyone else.

- [ ] **Step 1: Mockup artifact:** tabs Conversations, Suggestions, Flagged. **Stop for owner approval.**
- [ ] **Step 2: Write failing tests.**
  - `non_owner_refused_on_every_function_and_page`.
  - `all_conversations_listed_with_role_and_date`.
  - `approve_keeps_gemma_reply_and_owner_text_separately_with_author`: memory D10 keeps authorship.
  - `approve_refuses_text_or_question_with_identifiers`: the question reaches other users, so the owner can reword it.
  - `approved_qa_reaches_other_runs_of_topic_cited_owner_reviewed`.
  - `rejected_qa_is_never_retrieved`; `flagged_lists_check_flags_and_thumbs_down`; `eval_conversations_are_marked`.
- [ ] **Step 3: Run; expect failure.**
- [ ] **Step 4: Implement.** The chat sweep fills `topic_qas.embedding`.
- [ ] **Step 5: Run all house checks; expect PASS.**
- [ ] **Step 6: Commit, review, PR C,** wait for "merge it".

## PR D · Stage CH6: prior and reconciliation

### Task 12: Prior, reconciliation, belief passages, scorecard

**Files:** Create `src/chat/prior.rs`; Modify `src/chat/job.rs`, `src/app/chats_page.rs` (Prior vs evidence tab); Test `tests/chat_prior.rs`

**Interfaces:**
- Produces: `prior::prior_request(question) -> GemmaRequest` (deliberately not grounded, used only here); `prior::parse_prior(&Value) -> Result<Vec<PriorClaim>, _>`, with `PriorClaim { claim, numbers, studies }`, capped at 8; `prior::reconcile_claim_request(claim, &GroundedPassages) -> GemmaRequest`; `prior::check_reconciliation(...) -> Reconciled`; `prior::belief_passage(...) -> Option<NewPassage>`; `prior::scorecard(...) -> Value`.
- Runs in `chat_loop`, one Gemma call per tick, **only when no turn is pending** (a user's reply always comes first). Priors are 120 s limit, `format: "json"`.

- [ ] **Step 1: Write failing tests.**
  - `prior_written_once_for_a_running_full_run`: not for Quick, not after 15 minutes, not when `CHAT_PRIOR=off`; two instances give one row.
  - `bad_json_is_failed_and_the_run_is_untouched`; `gemma_down_never_touches_the_run`.
  - `claim_with_no_passage_is_not_in_evidence_without_calling_gemma`.
  - `corrected_claim_becomes_a_cited_belief_passage`; `supported_claim_becomes_matches_belief`.
  - `claim_text_with_numbers_gets_no_belief` (P6).
  - `untraced_evidence_version_gets_no_belief`; `unknown_settling_citation_gets_no_belief`.
  - `not_in_evidence_never_becomes_a_passage`; `raw_prior_is_never_retrieved`.
  - `belief_inherits_settling_comparisons`.
  - `scorecard_counts_and_invented_references`: a prior study matching no run reference counts as invented.
  - `prior_tab_owner_only`; `yields_when_turn_pending`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all house checks; expect PASS.**
- [ ] **Step 5: Commit, review, PR D,** wait for "merge it".

## PR E · Stage CH7: evaluation and switch-on

### Task 13: Owner evaluation and go-live

**Files:** Create `src/chat/eval.rs`; Modify `src/app/chats_page.rs`, `infra/README.md`; Test `tests/chat_eval.rs`

**Interfaces:** Produces `eval::cases(answer: &Value) -> Vec<EvalCase>` (about 20: factual, how sure, personal, warning symptom, off-topic, other comparison); `eval::score(case, turn) -> EvalScore`; `eval::start(state, user, run_id)`, owner only, runs turns one at a time on an `is_eval` conversation, outside the daily limit.

- [ ] **Step 1: Write failing tests:** `cases_cover_every_category`; `score_each_category`; `eval_is_owner_only_and_outside_daily_limit`; `eval_yields_to_real_turns`.
- [ ] **Step 2: Run; expect failure.**
- [ ] **Step 3: Implement,** with a "Run test questions" action and a summary in the owner view. House checks; PR E; "merge it".
- [ ] **Step 4: Owner switches on.** One step at a time: pull check `docker exec ollama-s9os-ollama-1 ollama list`; set `CHAT=on`; Re-run Deploy. Only the owner has a login at this stage; the sign-up notice (C4) comes before anyone else is added.
- [ ] **Step 5: Measure and calibrate.** Check `chat_prep` shows all 7 records `ok`; time the vector fill against the "about a minute" aim (roughly 100+ passages per run); record warm and cold reply times with both models in use. Run the evaluation on the bilateral TAR run, then on one unrelated run. Set `CHAT_MIN_SCORE` from the results: the TAR question on the unrelated run must be not covered, and in-scope questions must not be.
- [ ] **Step 6: Owner smoke on phone,** reviewing the first reconciliations (spec §14). Update the build log, handover and `.10x/status.md`. Tag a release: the owner does it.

---

## Known limits carried forward

- Warning-symptom and personal detection is English keywords only. A question in Hindi or Kannada gets no fixed line, and retrieval depends on meaning alone.
- Number checks work on the whole reply, so a number taken from cited passage [2] but attached to the wrong claim passes.
- Voice follows after CH7 (C3). Temporal T1 bugs carry into era-derived text until the T1 fix; passages then need a rebuild alongside the planned ledger rebuild.
