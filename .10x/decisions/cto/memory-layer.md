# CTO — memory-layer

- **Build, not buy.** Evaluated waku-agent (MIT harness + ELv2 hosted; Python; LLM-consolidated facts) and MemPalace (MIT; Python; verbatim + ChromaDB/pgvector). Neither fits a Rust Cloud Run service; borrow ideas only (verbatim storage, validity windows, gate). No code reused, no attribution needed.
- **Why now:** each Full run costs ₹250–400 and today is used once and deleted at 12 months. Memory turns spend into a compounding, versioned evidence map per topic, cheaper repeat questions, and an owner-curated expert layer.
- **Infra:** embeddings on owner's existing Hostinger VPS (Mumbai) — no new cloud spend. Postgres (Supabase) for everything else; pgvector already enabled.
- **Risk posture:** behind MEMORY=off until calibrated; engine frozen; no step can affect a run.
- **Sequencing constraint (owner, 1 Nov 22:47):** the Expert Opinion UI (temporal T4) is still being built. Back-end stages M0–M3 run in parallel; UI stages M4–M5 start only after T4 is merged to main.
