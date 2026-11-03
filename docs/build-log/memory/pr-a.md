# Incisive memory layer — build log, PR A (back end)

**Branch:** `memory-layer` · **Base:** main 7ff75a0 · **Head:** 47fb2df · **Dates:** 1–2 Nov 2026
**Spec:** docs/superpowers/specs/2026-11-01-incisive-memory-layer-design.md · **Plan:** docs/superpowers/plans/2026-11-01-incisive-memory-layer.md · **ADR:** .10x/adrs/001

## What PR A adds (nothing visible to users; MEMORY=off by default)
- Migration 0013: topics, evidence_records, ledger_studies, ledger_claims, topic_verdicts, owner_revisions, topic_changes, memory_jobs.
- `MEMORY` switch; Gemma settings (`GEMMA_URL`, `GEMMA_AUTH`, `EMBED_MODEL`); match thresholds (placeholders until calibration).
- Pure extraction of topic label, arms, studies and per-era claims from a stored Full answer + Expert Opinion.
- Topic matcher (meaning + arms + words), store, post-opinion job and queue loop, backfill command.
- Embeddings from the owner's Gemma VPS with full-text fallback; background fill of missing vectors.

## Stage-by-stage review outcomes
| Task | Result | Fix rounds | Notable findings fixed |
|---|---|---|---|
| 1 migration | clean | 0 | — |
| 2 switch | clean | 0 | — |
| 3 extract | clean | 0 | confirmed natural-scale estimates and distinct-study counts |
| 4 matcher | fixed | 1 | used the candidate's word score instead of each topic's |
| 5 store | fixed | 1 | studies with no year were silently dropped; stuck jobs |
| 6 job + loop | fixed | 1 | duplicate record race across instances; transient failures were final |
| 7 backfill | fixed | 1 | command ran migrations; untested status filter |
| 8 embeddings | fixed | 1 | swept records used a different text; topics never got vectors |
| Final review (whole branch) | fixed | 1 wave | **claims ledger dropped estimates** (hernia 12→10, achalasia 7→6); full-text search couldn't find a run's own topic; relative rank; startup order; poison-record sweep; one sweep across instances; https-only Gemma URL |

Final scoped re-review: all findings addressed; ready to merge behind MEMORY=off.

## Owner actions
1. **Before merge:** Supabase SQL editor — `SELECT e.extversion, n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE extname='vector'; SHOW search_path;` (need ≥ 0.5, schema on path).
2. After merge: create `gemma-auth` secret (commands in infra/README.md).
3. After freezing the Expert Opinion: add `crate::memory::job::on_temporal_done(&state, run_id).await` after a Done finish in runs/temporal.rs (small PR, with the deploy workflow wiring).
4. Smoke: MEMORY=on + TEMPORAL=on → `memory_backfill` → check records/claims in SQL.

## Parked for PR B
GEMMA_URL backslash parsing; advisory-lock drop guard; per-record embedding attempt cap.

## Live (2 Nov 2026, 22:30 IST)
- PR #18 merged (55e1b7b), deployed; tagged **v0.7.0** (owner). pgvector 0.8.2 in `public` confirmed. `gemma-auth` secret + binding created; repo variables `GEMMA_URL`, `HAS_GEMMA_KEY=true`, then `MEMORY=on` + Deploy re-run.
- Backfill (SQL equivalent of `memory_backfill`): 7 Full runs with a done Expert Opinion → 7 jobs done, 0 errors.
- Result: 7 records in 6 topics; every record and topic has a Gemma vector. Two umbilical/small ventral runs joined one topic (28 studies, 27 estimates). Bilateral TAR kept separate.
- Calibration data point 1 (owner): "Neoadjuvant vs adjuvant chemotherapy for locally advanced (r…)" and "Neoadjuvant vs adjuvant therapy for rectal cancer: survival…" are **different questions** — kept apart correctly at the default thresholds.
