# Handoff

**From:** Discovery → **To:** Brainstorming (all roles)

Feature `memory-layer`: a hybrid memory over stored Incisive runs (verbatim Full answer + Expert Opinion pairs, topic index, claim ledger from the temporal era table, gate before runs, owner's expert layer). Design principles agreed in chat 1 Nov 2026: MemPalace-style verbatim storage, Waku-style gate, built natively in Rust + Postgres; no code reused from either.

Key facts for design: runs cascade-delete at 365 days (conflicts with long-term memory); PICO exists at framing but is not stored; table_json already holds the era table; no RLS; pgvector extension enabled but unused.
