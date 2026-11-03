-- The introduction agent: one short introduction to the question, written by
-- its own call while the run works. Additive: a new table only. Delete and
-- the 12-month retention cascade from runs. Scoping by user goes through the
-- runs join. Never read by the engine, the memory layer or the chat.
CREATE TABLE incisive.run_intros (
    run_id UUID PRIMARY KEY REFERENCES incisive.runs(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','done','failed','skipped','cap_blocked')),
    -- A fixed reason kind (e.g. "time", "truncated"); never text from the model.
    reason TEXT,
    -- The introduction, only when status is 'done'.
    text TEXT,
    cost_paise BIGINT NOT NULL DEFAULT 0,
    -- The job's own spend request id: its reservation and charge are booked
    -- under it (never under the run), so recovery can release a leaked one.
    request_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

-- Recovery looks for pending rows older than a few minutes.
CREATE INDEX run_intros_pending_idx ON incisive.run_intros (created_at)
    WHERE status = 'pending';
