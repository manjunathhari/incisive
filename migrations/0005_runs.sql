-- Evidence-agent runs. A run is one accepted question and its background work.
-- The question and answer are private to the owning user; money is integer paise.
CREATE TABLE incisive.runs (
    id UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    question TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('quick','focused')),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','running','done','partial','failed','interrupted')),
    cost_paise BIGINT NOT NULL DEFAULT 0,
    limit_paise BIGINT NOT NULL,
    error_kind TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);

CREATE INDEX runs_user_created_idx ON incisive.runs (user_id, created_at DESC);

-- One active run per user, enforced by the database.
CREATE UNIQUE INDEX runs_one_active_per_user ON incisive.runs (user_id)
    WHERE status IN ('queued','running');

CREATE TABLE incisive.run_steps (
    run_id UUID NOT NULL REFERENCES incisive.runs(id) ON DELETE CASCADE,
    seq INT NOT NULL,
    at TIMESTAMPTZ NOT NULL DEFAULT now(),
    kind TEXT NOT NULL,
    line TEXT NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE incisive.run_answers (
    run_id UUID PRIMARY KEY REFERENCES incisive.runs(id) ON DELETE CASCADE,
    body JSONB NOT NULL
);

-- Public bibliographic data only; no user linkage.
CREATE TABLE incisive.paper_cache (
    pmid TEXT PRIMARY KEY,
    doi TEXT,
    title TEXT NOT NULL,
    authors TEXT NOT NULL,
    journal TEXT,
    year INT,
    pub_types TEXT[] NOT NULL,
    abstract TEXT,
    oa_url TEXT,
    retracted BOOLEAN NOT NULL DEFAULT FALSE,
    fetched_at TIMESTAMPTZ NOT NULL
);
