-- Stage S1: the half-written reply of a writer, so the page can show it while
-- it is being written. Additive: a new table only. Holds reply text, like
-- run_answers, and is scoped to the run's owner through the runs join. Delete
-- and the 12-month retention cascade from runs. A row lives only while its
-- writer works: it is deleted when the final is saved or the run ends any other
-- way, and recovery removes stale ones. Never read by the engine, the memory
-- layer or the chat.
CREATE TABLE incisive.run_drafts (
    run_id UUID NOT NULL REFERENCES incisive.runs(id) ON DELETE CASCADE,
    part TEXT NOT NULL CHECK (part IN ('answer','opinion')),
    text TEXT NOT NULL,
    chars INTEGER NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, part)
);
