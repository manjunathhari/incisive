-- Tamper-evident audit log: a hash chain. Each row's `hash` covers its own
-- fields plus the previous row's hash, so editing or deleting a row is
-- detected by `audit::verify_chain`.
--
-- The REVOKE documents intent only. Supabase connects the app as the database
-- owner, and an owner is unaffected by REVOKE ... FROM PUBLIC. The protection
-- is tamper DETECTION, not prevention (see src/audit.rs and infra/README.md).
CREATE TABLE incisive.audit (
    seq BIGINT PRIMARY KEY,
    at TIMESTAMPTZ NOT NULL,
    request_id UUID,
    user_id BIGINT,
    event TEXT NOT NULL,
    detail JSONB NOT NULL,
    prev_hash TEXT NOT NULL,
    hash TEXT NOT NULL UNIQUE
);

REVOKE UPDATE, DELETE ON incisive.audit FROM PUBLIC;
