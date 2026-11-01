# Staff Engineer — memory-layer

- Follow `temporal/` pattern: pure modules separate from I/O; plain `sqlx::query`; `sqlx::Result` returns; codes not text in errors.
- Config parsed like TEMPORAL (strict on/off). Secrets as `SecretString`, never in Debug.
- Owner checks in domain fns (ask.rs:646 pattern), server fns map NotOwner → 403.
- Tests: integration via TEST_DATABASE_URL (skip if unset), wiremock for HTTP, fixtures from tests/fixtures/temporal.
- Every AppState literal in tests needs the new fields (memory, embedder).
- No change to engine files beyond the one enqueue call in runs/temporal.rs (post-run).
