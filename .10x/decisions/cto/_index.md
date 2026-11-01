# CTO — cross-cutting

- [DISCOVERED] Stack: Rust workspace (edition 2024): `claude-client` + `incisive-app` (Leptos + Axum, sqlx 0.8).
- [DISCOVERED] Hosting: Cloud Run asia-south1 + Supabase Postgres (Mumbai). Claude Messages API, own agent loop (no SDK).
- [DISCOVERED] Engine frozen at v0.4.0 by owner rule: no engine changes without a clear, owner-approved reason. New work is additive, behind switches.
- [DISCOVERED] Cost: Max-plan API credit; daily caps per user (fellow ₹400, surgeon ₹800, owner ₹3,000).

## Active features
- memory-layer (Phase 0, brainstorming)
