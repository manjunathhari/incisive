# Staff Engineer — cross-cutting

- [DISCOVERED] Plain `sqlx::query(...)` strings in store modules (`runs/store.rs`, `runs/temporal.rs`, …); no compile-time `query!` macros.
- [DISCOVERED] Switches read once in `src/config.rs` (`Config::from_env`), strict values (unknown value = config error).
- [DISCOVERED] Tests: integration tests in `crates/incisive-app/tests/` against real Postgres via `TEST_DATABASE_URL` (skip if unset); LLM faked with `ScriptedLlm`; fixtures under `tests/fixtures/`.
- [DISCOVERED] House rules: flag never delete; no step may fail or alter a run; LLM returns JSON, Rust renders; never log question or reply text; builder + independent reviewer; fmt, clippy, full suite, release hydrate build; merge only on owner's "merge it".
