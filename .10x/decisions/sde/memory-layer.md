# SDE — memory-layer (PR A built 2 Nov 2026)

Tasks 1–8 on branch memory-layer (main 7ff75a0 → 47fb2df). Per-task builder + reviewer; final whole-branch review (opus) + one fix wave.
- src/memory/{settings,extract,matcher,store,job,embed}.rs; migrations/0013_memory.sql; src/bin/memory_backfill.rs; infra/README.md section.
- Deviations from plan (rulings): no call from runs/temporal.rs yet (`on_temporal_done` exists; owner adds after freezing Expert Opinion); deploy workflow unchanged until `gemma-auth` secret exists; Candidate has no fts_rank (per-topic rank in TopicFeatures); claims keyed (record_id, group_id, era_id); embed_text persisted; ledger_studies year/era nullable; unique record per run; store failures retry ≤3; FTS OR-query + absolute rank; advisory-locked sweep; GEMMA_URL https-only (loopback excepted).
- Tech debt (parked for PR B): GEMMA_URL backslash parse; advisory-lock drop guard; per-record embed attempt cap.
