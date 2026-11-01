-- Temporal T3: the Expert Opinion of a Full answer, stored 1:1 with its run
-- so the answer and the opinion are kept as a linked pair, findable by topic
-- and year (owner decision D2). Additive: a new table only. Delete and the
-- 12-month retention cascade from runs. Scoping by user goes through the
-- runs join, as for History. Never holds anything the run row needs.
CREATE TABLE incisive.run_temporal (
    run_id UUID PRIMARY KEY REFERENCES incisive.runs(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','done','failed','skipped','cap_blocked')),
    -- A fixed reason kind (e.g. "not enough primary studies", "time"); never text from the model.
    reason TEXT,
    -- The writer's short topic label, e.g. "Robotic vs laparoscopic inguinal hernia repair".
    topic TEXT,
    -- The year the latest era ends in (the run's year).
    evidence_year INT,
    table_json JSONB,
    board_json JSONB,
    flags JSONB,
    cost_paise BIGINT NOT NULL DEFAULT 0,
    -- The job's own spend request id: its reservation and charges are booked
    -- under it (never under the run), so recovery can release a leaked one.
    request_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX run_temporal_topic_idx ON incisive.run_temporal (topic);
CREATE INDEX run_temporal_evidence_year_idx ON incisive.run_temporal (evidence_year);
