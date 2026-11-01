# Incisive memory layer: design

**Date:** 1 Nov 2026 · **Status:** approved in brainstorming, awaiting owner review of this written spec
**Branch:** `memory-layer` · **Team state:** `.10x/` (decisions D1–D9, A1–A5)

## 1. Purpose

Every Full run produces an evidence answer and an Expert Opinion that cost real money and time, and today each run is used once and deleted at 12 months. The memory layer keeps them, groups them by topic, and lets them grow: each new run on a topic adds its studies to that topic's era history, so the temporal picture improves with use instead of restarting from zero.

It also lets a repeat question open a stored answer for free, and gives the owner a curated, human-written verdict per topic: the clean set for any future Gemma training.

**Principles (from comparing waku-agent and MemPalace, 1 Nov 2026):**
- **Verbatim, never paraphrased** (MemPalace): the stored record is the run's own answer, Expert Opinion and era table, byte for byte. Nothing is summarised by a model.
- **Check memory before doing work** (Waku's gate), done in code, not with a model.
- **Time is first-class** (our temporal framework): studies are placed in eras; claims are kept per era and per version, superseded rather than overwritten.
- **Built natively** in Rust + Postgres. No code from either project.

## 2. Owner decisions

| # | Decision |
|---|---|
| D1 | A user's private run history keeps the 12-month delete. On save, a de-identified copy of the evidence pair goes into a permanent library with no link to who asked. Invitees are told at sign-up. |
| D2 | Topic grouping is fully automatic, by code. |
| D3 | The owner can merge and split topics; every change is logged and reversible. |
| D4 | Gate: on a strong match, open the stored answer + Expert Opinion at no cost, with "Update with newer evidence". |
| D5 | Only the owner can browse the shared library. |
| D6 | Every user's gate matches against all stored records, de-identified. |
| D7 | Expert layer: owner verdict (adopt / selective / not yet) plus a note, per topic, dated and versioned. |
| D8 | Full-text and meaning-search both in v1. |
| D9 | "Update" in v1 is a normal Full run; the engine stays frozen. A date-limited search is a later, separately approved engine change. |
| D10 | Every Claude-written answer and Expert Opinion is kept in full. The owner can add an owner-reviewed version in his own words, stored separately. Every stored text carries its author (`claude` or `owner`) so a training set can be extracted cleanly. Claude-written text is used for retrieval (memory, gate, future chat), not as a training target, unless Anthropic gives written permission. |
| A1/A2 | Embeddings: `embeddinggemma` (768 dimensions) on the owner's Hostinger VPS (Mumbai), via Ollama behind Traefik: `https://ollama-s9os.srv2117610.hstgr.cloud/api/embed`, basic auth, only `/api/embed` and `/api/chat` routed. Verified 1 Nov 2026. |

## 3. Scope

**In v1:** Full runs only (only they have an Expert Opinion). Topics, evidence records, ledger, gate on the Ask page, owner Topics area (list, topic page, verdicts, owner-reviewed versions, merge/split), full-text + meaning search, backfill of existing Full runs that have a temporal row.

**Not in v1:** Quick runs; date-limited "update" search (D9); "Ask this evidence" chat; any Gemma training; per-surgeon private notes; changes to the run engine, its prompts, schemas, money or time logic.

## 4. Data model (migration `0013_memory.sql`, additive)

All tables in schema `incisive`. No existing table changes.

**`topics`**
- `id BIGSERIAL PK`, `label TEXT NOT NULL` (from the Expert Opinion `board.topic`; owner can rename)
- `pairs JSONB NOT NULL`: sorted, normalised intervention–comparator pairs from the era table
- `embedding vector(768) NULL`, `embed_model TEXT NULL`
- `merged_into BIGINT NULL REFERENCES topics(id)` (a merged topic stays as a pointer, never deleted)
- `created_at`, `updated_at`

**`evidence_records`**: the permanent library copy
- `id BIGSERIAL PK`, `topic_id BIGINT NOT NULL REFERENCES topics(id)`
- `version INT NOT NULL` (per topic, 1, 2, 3…), unique `(topic_id, version)`
- `source_run_id UUID NULL REFERENCES runs(id) ON DELETE SET NULL`: **not** cascade. When the run expires the link is cleared and the record can no longer be traced to a user.
- `question TEXT NOT NULL` (kept for the owner's library and search; never shown to other users, see §6)
- `answer JSONB NOT NULL` (AnswerV3, byte-identical copy), `answer_version SMALLINT NOT NULL`
- `board JSONB NOT NULL`, `era_table JSONB NOT NULL` (copies from `run_temporal`)
- `evidence_through DATE NOT NULL`: the run's date (the latest the search could have seen)
- `fts tsvector GENERATED ALWAYS AS (...) STORED` over question, topic label, verdict line and outcome names; GIN index
- `embedding vector(768) NULL`, `embed_model TEXT NULL`, `embedded_at TIMESTAMPTZ NULL`
- `created_at`
- Records are immutable. Corrections are new versions.

**`ledger_studies`**: one row per study per topic (the part that grows)
- `topic_id`, `pmid TEXT`, `pub_year INT`, `treatment_start INT NULL`, `treatment_end INT NULL`, `years_assumed BOOL`, `era_id TEXT`, `first_record_id BIGINT` (the record that first found it), `last_record_id BIGINT`
- PK `(topic_id, pmid)`; a study found again is counted once, `last_record_id` updated.

**`ledger_claims`**: per outcome, per era, per record
- `record_id`, `topic_id`, `outcome TEXT`, `intervention TEXT`, `comparator TEXT`, `era_id TEXT`, `era_start INT`, `era_end INT`, `estimate NUMERIC NULL`, `lo NUMERIC NULL`, `hi NUMERIC NULL`, `effect_type TEXT`, `n_studies INT`, `label TEXT` (the trajectory label)
- PK `(record_id, outcome, intervention, comparator, era_id)`
- Taken directly from the stored `TemporalTable`; never recomputed differently.

**`topic_verdicts`**: the expert layer, append-only
- `id`, `topic_id`, `verdict TEXT CHECK IN ('adopt','selective','not_yet')`, `note TEXT`, `based_on_record_id BIGINT NULL`, `author_user_id BIGINT`, `created_at`
- The current verdict is the newest row. Nothing is updated or deleted.

**`owner_revisions`**: owner-reviewed versions (D10), append-only
- `id BIGSERIAL PK`, `record_id BIGINT NOT NULL REFERENCES evidence_records(id)`
- `part TEXT CHECK IN ('answer_bottom_line','answer_section','opinion','opinion_part')`, `part_key TEXT NULL` (which section or part, e.g. an outcome name or opinion part number)
- `claude_text TEXT NOT NULL` (the exact Claude passage being revised, copied at edit time)
- `owner_text TEXT NOT NULL` (the owner's own words), `author TEXT NOT NULL DEFAULT 'owner'`
- `author_user_id BIGINT`, `created_at`; a newer row for the same part supersedes the older, nothing is overwritten.
- The stored answer view shows the newest owner version where one exists, labelled "Reviewed by the owner", with the original Claude text one tap away.

**Authorship:** every stored text is tagged by source. Claude-written: `evidence_records.answer`, `board`; owner-written: `topic_verdicts`, `owner_revisions.owner_text`; code-made: `era_table`, ledger rows. A training export (later) reads owner-written text only, unless written permission from Anthropic is recorded.

**`topic_changes`**: merge/split log
- `id`, `kind TEXT CHECK IN ('merge','split','rename','undo')`, `payload JSONB` (enough to reverse it), `author_user_id`, `created_at`

**`memory_jobs`**: post-run work queue
- `run_id UUID PK REFERENCES runs(id) ON DELETE CASCADE`, `status` (pending/done/failed/skipped), `attempts INT`, `last_error_kind TEXT` (a code, never text from the answer), `record_id BIGINT NULL`, timestamps

**Access rules:** row-level security enabled on every new table with no policies for the Data API roles (the Data API is off; this is a second lock). The app connects as the owner role and filters in code as it does today.

**Indexes:** GIN on `fts`; HNSW (cosine) on `evidence_records.embedding` and `topics.embedding`; B-tree on `topic_id`, `evidence_through`.

## 5. Components

All new code in `crates/incisive-app/src/memory/`, pure parts separate from I/O, following the `temporal/` pattern.

| Unit | Does | Depends on |
|---|---|---|
| `memory::extract` (pure) | From a stored AnswerV3 + TemporalTable + board: topic label, normalised pairs, ledger studies, ledger claims, FTS text, embedding text | answer and temporal types only |
| `memory::matcher` (pure) | Scores a candidate against topics: pair overlap (Jaccard) + vector cosine + FTS rank, combined; returns best topic and score | nothing |
| `memory::embed` | HTTP client for Ollama `/api/embed` with basic auth, 5 s timeout, model name check, 768-dim check | config, `reqwest` |
| `memory::store` | All SQL for the new tables. Vectors are bound as text and cast (`$1::vector`), so no new crate is needed | sqlx |
| `memory::job` | After a Full run's temporal row is `done`: extract → match or create topic → insert record + ledger in one transaction → embed (best effort) | the above |
| `memory::gate` | For a question text: FTS + query embedding (if VPS reachable) against records → best topic + score → `Strong` / `None` | matcher, store, embed |
| `memory::backfill` | One-off command: queue every existing Full run with a `done` temporal row | job |
| UI | Ask-page gate card; stored-answer view (topic label, not question); owner Topics list, topic page, verdict form, merge/split | server functions |

## 6. Flows

**After a Full run** (only when `MEMORY=on`):
1. The temporal job finishes as today. If it is `done`, a `memory_jobs` row is inserted. Skipped or failed temporal → memory job `skipped` (no Expert Opinion, nothing to store).
2. The memory worker takes pending jobs: extract, then match.
   - Match score ≥ `MEMORY_TOPIC_MATCH` (default strict, tuned in Stage 6) → join that topic as the next version.
   - Otherwise → create a new topic.
3. One transaction writes the record, ledger studies (upsert by PMID) and ledger claims.
4. Embeddings for the record (and topic, if new) are requested; on failure they stay NULL and a sweep retries later.
5. Errors mark the job failed with a code; retried up to 3 times. The run, its answer and its Expert Opinion are never touched.

**When someone asks a question** (gate, `MEMORY=on`):
1. Before the run is queued, `memory::gate` scores the question against stored records: FTS always; vector only if the VPS answers within 2 s.
2. Score ≥ `MEMORY_GATE_MATCH` → the Ask page shows: *"An answer on this topic already exists (evidence through <Mon YYYY>)."* with **Open it** (free) and **Update with newer evidence** (normal Full run, normal cost and caps). The user may also dismiss and run as usual.
3. Otherwise the run proceeds exactly as today.
4. The stored-answer view shows the **topic label**, the answer and the Expert Opinion. It never shows the original question or anything identifying who asked.

**Owner Topics area** (role `owner` only; 403 for others):
- List: topics with version count, latest evidence date, current verdict; search box (FTS + meaning).
- Topic page: versions (each opens the stored answer), the ledger (studies by era, claims by era, across versions), verdict history, verdict form, rename, merge into another topic, split selected records into a new topic, undo the last change.

**Retention:** `retention_loop` is unchanged. Runs still delete at 365 days; `memory_jobs` cascades; `evidence_records.source_run_id` becomes NULL; records, ledger and verdicts stay.

## 7. Configuration

Read in `config.rs` with the existing strict style:
- `MEMORY = off | on` (default `off`; unknown value is a config error)
- `GEMMA_URL` (e.g. `https://ollama-s9os.srv2117610.hstgr.cloud`), `GEMMA_AUTH` from Secret Manager secret `gemma-auth` (`incisive:<password>`), `EMBED_MODEL` (default `embeddinggemma`)
- `MEMORY_TOPIC_MATCH`, `MEMORY_GATE_MATCH` (floats, defaults set in Stage 6)
- `MEMORY=on` with `GEMMA_URL` unset is allowed: full-text only.
- `MEMORY=on` needs `TEMPORAL=on`: a record is only made from a run whose Expert Opinion is `done`. With `TEMPORAL=off`, the gate still works on existing records but no new ones are added; config logs a warning, not an error.

## 8. Error handling and safety

- Flag, never delete. Records are immutable; topics merge by pointer.
- No memory step can fail, delay or alter a run. The gate has a hard 2 s budget for the VPS and falls back to full-text.
- Question and answer text never logged; job errors stored as codes.
- The Gemma credential lives only in Secret Manager and process memory; never in logs, URLs or the repo.
- Owner-only routes check role on the server, not only in the UI.
- Other users never see who asked or the original question.
- Rollback: `MEMORY=off` and re-run Deploy. Tables stay; nothing is lost.

## 9. Testing

- **Pure units:** `extract` and `matcher` against the three fixtures (cholecystitis, hernia, achalasia) with golden expectations: topic label, pairs, ledger studies and claims identical to the stored era table.
- **Ledger merge:** two versions of one topic → shared PMIDs counted once, claims kept per record.
- **Store integration** (real Postgres via `TEST_DATABASE_URL`, as existing tests): job end to end; `source_run_id` cleared when a run is deleted while the record survives; verdicts append-only; merge/split/undo round-trip.
- **Embed client:** fake HTTP server for success, 401, timeout, wrong dimension → no panic, NULL embedding, FTS still works.
- **Gate calibration (Stage 6):** gold-set questions + paraphrases + unrelated questions → paraphrases find their topic, unrelated ones do not; thresholds chosen and recorded.
- **Access:** non-owner gets 403 on every Topics route; the stored-answer view contains no question text.
- House checks: fmt, clippy (workspace and hydrate), full suite, release hydrate build; builder + independent reviewer; owner smoke on the live site with `MEMORY=on`; merge only on "merge it".

## 10. Stages

| Stage | Delivers | Visible to users |
|---|---|---|
| M0 | Migration 0013 + config switches (off) | No |
| M1 | `extract` + `matcher` (pure) with fixture tests | No |
| M2 | `store` + `job` + backfill of existing runs | No |
| M3 | `embed` client + Secret Manager wiring + embedding sweep | No |
| M4 | Owner Topics area (list, topic page, verdicts, owner-reviewed versions, merge/split) | Owner only |
| M5 | Gate on the Ask page + stored-answer view | Everyone, when on |
| M6 | Gate calibration on the gold set, threshold defaults | — |

Each stage is its own PR, behind `MEMORY=off` until M6 is accepted.

## 11. Open items (not blocking v1)

- Sign-up notice wording for D1 (owner to approve before inviting users).
- Date-limited "update" search (D9), as a separate engine change.
- "Ask this evidence" chat on Gemma, reading only stored records.
- Training-data export of owner-written text (verdicts, notes, owner revisions). Claude-written answers are not training targets under Anthropic's terms; the owner may request written permission from Anthropic for a narrow surgical explanation model, and if granted, the export can include reviewed Claude text.
