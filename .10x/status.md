# Status

| Feature | Phase | Updated |
|---|---|---|
| memory-layer | PR A (Tasks 1–8) built + reviewed; awaiting owner pgvector check, PR, "merge it" | 2026-11-02 |

## Open questions for owner
1. ~~Retention~~ — decided A (de-identified permanent library copy).
2. ~~Topic grouping~~ — automatic by code (D2); owner merge/split view (D3).
3. ~~Gate, visibility, expert layer, search~~ — D4–D8 decided.
4. ~~Embedding host~~ — owner VPS Ollama + embeddinggemma behind Traefik (A1). Waiting on owner setup (subdomain).

## Task list (plan)
- [x] T1 migration 0013 · [x] T2 MEMORY switch · [x] T3 extract · [x] T4 matcher · [x] T5 store · [x] T6 job+loop (hook pending owner) · [x] T7 backfill · [x] T8 embeddings
- [ ] T9 topics domain · [ ] T10 topics pages (after temporal T4 on main)
- [ ] T11 gate · [ ] T12 Ask card + stored page · [ ] T13 calibration
