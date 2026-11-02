# Senior Engineer — memory-layer

- Tricky parts: (1) deriving claims from EraCell (pooled vs estimate) — golden test from current `build` output; (2) concurrent claim — SKIP LOCKED + stale `running` reclaim after 10 min; (3) split rebuilds ledgers from stored era tables (only place ledgers are rewritten); (4) vector binding as text with `::vector` cast; (5) gate latency — 2 s embed budget, never block submit on errors.
- Keep `process` idempotent: memory_jobs PK on run_id + ON CONFLICT DO NOTHING; a retried job after a committed record must not create a second one (check for an existing record with source_run_id first).
