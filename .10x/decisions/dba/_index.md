# DBA — cross-cutting

- [DISCOVERED] Schema `incisive`; migrations 0001–0012 in `migrations/`, applied by sqlx migrate.
- [DISCOVERED] Retention: `retention_loop` (main.rs) deletes runs older than 365 days every 24 h; answers, steps and temporal rows go by cascade. paper_cache pruned at 90 days.
- [DISCOVERED] No row-level security; scoping is in app code (`WHERE user_id = $1`). Data API off.
- [DISCOVERED] `vector` extension already created.
