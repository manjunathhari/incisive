# Engineering Manager — memory-layer

Plan: `docs/superpowers/plans/2026-11-01-incisive-memory-layer.md` (13 tasks).

| Stage | Tasks | PR | Can start |
|---|---|---|---|
| M0 schema + switch | 1–2 | PR A (M0–M3 together) | now |
| M1 extract + matcher | 3–4 | PR A | now |
| M2 store + job + backfill | 5–7 | PR A | now |
| M3 embeddings + secret | 8 | PR A | now (owner creates `gemma-auth` secret) |
| M4 owner Topics | 9–10 | PR B | after temporal T4 merged to main; mockup approval |
| M5 gate | 11–12 | PR C | after PR B; mockup approval |
| M6 calibration | 13 | PR C | owner seeds runs |

Risks: Rust build times (CI ~long) — batch M0–M3 into one PR; T4 overlap on app.rs/ask_page.rs — rebase before M4; calibration needs enough topics (owner cost).
