# Handoff

**From:** SDE/QA/Security (PR A) → **To:** Owner + DevOps

PR A (Tasks 1–8) is complete on `memory-layer` (head 47fb2df), all reviews closed. Before merge: owner runs the pgvector check in Supabase. After merge (MEMORY stays off): owner creates `gemma-auth` secret (infra/README.md), then a small PR wires the deploy workflow and adds the `on_temporal_done` call in runs/temporal.rs once the Expert Opinion is frozen. Then smoke: MEMORY=on + TEMPORAL=on, run `memory_backfill`, check records in SQL.
Next build: PR B (Tasks 9–10, owner Topics; mockup first) + parked hardening.

## Update 2 Nov 2026, 20:45 IST
- Rebased onto main 05bc998 (v0.6.1). Full suite 912 passed incl. temporal_ui fingerprint; fmt + clippy clean.
- `on_temporal_done` wired in runs/temporal.rs after a Done finish (owner authorised; TEMPORAL=on live). Tests: queues only when MEMORY on and opinion done.
- T1 bugs (meta-analyses as primary, ratio-as-difference, robotic-vs-open in tables) flow into the ledger until fixed; spec §12 records the handling (bump TEMPORAL_VERSION; ledger rebuild command in PR B).
