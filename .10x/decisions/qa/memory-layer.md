# QA — memory-layer PR A

- Full suite green against Postgres 16 + pgvector (local). New: memory_schema 5, memory_store 12, memory_job 18, lib memory::{extract,matcher,embed,config} tests.
- Review Focus coverage: 1 extract errors ✓; 2 concurrent claim/process ✓; 3 merge chains ✓ (gate part in PR C); 4 junk VPS replies ✓; 5 run deleted mid-flight ✓ (in-transaction path tested).
- Claim count == extracted claims for all three fixtures; FTS-only self-join for all three fixtures.
- Not yet: owner smoke on live (needs merge + MEMORY=on + TEMPORAL=on + backfill).
