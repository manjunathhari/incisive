# DBA — memory-layer

- Migration 0013 (additive, unreleased until PR A merges; amended in place during build). Tables: topics, evidence_records, ledger_studies, ledger_claims, topic_verdicts, owner_revisions, topic_changes, memory_jobs. RLS on all, no policies.
- Keys: evidence_records unique partial (source_run_id); ledger_studies (topic_id, pmid); ledger_claims (record_id, group_id, era_id) + topic_id index; topics CHECK merged_into <> id.
- Indexes: GIN fts; HNSW vector_cosine_ops on evidence_records.embedding and topics.embedding (768).
- FKs: source_run_id ON DELETE SET NULL (records outlive runs); memory_jobs cascade with runs.
- PRE-MERGE (owner): confirm pgvector ≥ 0.5 and its schema on the app role's search_path in Supabase.
