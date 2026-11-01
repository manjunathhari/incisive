# Handoff

**From:** Planning (EM + Senior Engineer) → **To:** SDE (builder) + independent reviewer

Read: spec `docs/superpowers/specs/2026-11-01-incisive-memory-layer-design.md`, ADR-001, plan `docs/superpowers/plans/2026-11-01-incisive-memory-layer.md`.
Start with Tasks 1–8 (back end, PR A). Do not touch UI files until the temporal T4 UI is merged to main. MEMORY stays off in deploy until Task 13.
Owner action before Task 8 smoke: create Secret Manager secret `gemma-auth` (commands in plan Task 8).
