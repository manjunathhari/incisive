# ADR-001: Memory layer as verbatim evidence records in Postgres, with code-only matching and off-box embeddings

**Status:** Accepted
**Date:** 2026-11-01
**Feature:** memory-layer
**Author:** 10x-Team (Architect + Staff Engineer)

## Context
Full runs produce an AnswerV3 answer and an Expert Opinion (board + era table) that are expensive and deleted after 365 days by cascade from `incisive.runs`. The owner wants them kept, grouped by topic, growing over time, reusable by a gate before new runs, and annotated with his own verdicts and revisions. Constraints: Rust service on Cloud Run; Supabase Postgres (pgvector enabled, unused); engine frozen; no step may fail or alter a run; text never logged; owner's Hostinger VPS (Mumbai) runs Ollama with `embeddinggemma` (768-dim) behind Traefik + basic auth.

## Decision
1. **Storage:** new additive tables (migration 0013): `topics`, `evidence_records` (immutable, verbatim copies of answer/board/era table; `source_run_id ... ON DELETE SET NULL`), `ledger_studies`, `ledger_claims`, `topic_verdicts`, `owner_revisions`, `topic_changes`, `memory_jobs`. Nothing summarised by a model.
2. **Matching:** pure Rust scoring (pair overlap + vector cosine + FTS rank). No LLM calls for topic matching or the gate.
3. **Embeddings:** HTTP to the owner's VPS (`/api/embed`), asynchronous after save, 2 s budget at gate time, full-text fallback. Vectors bound as text and cast to `vector(768)`; no new crate.
4. **Processing:** a `memory_jobs` queue drained by a background loop, triggered after the temporal job is `done`; independent of the run worker.
5. **Switch:** `MEMORY=off|on` (default off); requires `TEMPORAL=on` to create records.

## Alternatives Considered
| Alternative | Pros | Cons | Why Not |
|---|---|---|---|
| Waku-style consolidation (LLM distils facts) | Compact | Paraphrase drift on numbers/qualifiers; extra cost; loses full answer | Medical evidence must stay verbatim |
| Run MemPalace as a Python sidecar | Mature retrieval, benchmarks | Personal-memory design (47 MCP tools), Python + ChromaDB beside Rust, more moving parts | Ideas fit, product doesn't |
| Embeddings inside the Rust container (ONNX) | One service | ~300 MB image, slower builds and cold starts | Builds already slow; VPS exists |
| Supabase built-in gte-small | Least code | Not Gemma, TypeScript edge function | Owner prefers Gemma; logic split |
| Remove the 365-day delete | Simplest | Keeps personal history indefinitely (DPDP) | D1 chose de-identified copy |
| LLM decides topic | Best with odd phrasing | Cost per question, unauditable | D2 chose code |

## Consequences
### Positive
- Evidence compounds per topic; repeat questions can be free; the owner's text becomes a clean, author-tagged set.
- Fully reversible: `MEMORY=off`, tables remain.
### Negative
- Topic matching by thresholds will sometimes merge or split wrongly (mitigated by owner merge/split/undo, strict defaults, M6 calibration).
- A second runtime dependency (VPS) for meaning-search.
- `question` text stays in the permanent record (owner-only visibility).
### Risks
- VPS down or credential leaked → FTS fallback; rotate password; credential only in Secret Manager.
- Hostinger panel edit restores the public port → ops note in SRE runbook.
- Answer schema changes (AnswerV3 → V4) → records store `answer_version`; extract handles versions explicitly.

## Dependencies
- Temporal Expert Opinion (T1–T3 merged; T4 UI in progress). UI stages M4–M5 wait for T4 on main.
- Constrains: any future chat ("Ask this evidence") reads `evidence_records` + `owner_revisions`; a future date-limited update search (D9) is a separate engine ADR.
