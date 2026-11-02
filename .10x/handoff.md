# Handoff

**From:** SDE/QA/Security (PR A) → **To:** Owner + DevOps

PR A (Tasks 1–8) is complete on `memory-layer` (head 47fb2df), all reviews closed. Before merge: owner runs the pgvector check in Supabase. After merge (MEMORY stays off): owner creates `gemma-auth` secret (infra/README.md), then a small PR wires the deploy workflow and adds the `on_temporal_done` call in runs/temporal.rs once the Expert Opinion is frozen. Then smoke: MEMORY=on + TEMPORAL=on, run `memory_backfill`, check records in SQL.
Next build: PR B (Tasks 9–10, owner Topics; mockup first) + parked hardening.
