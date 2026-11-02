-- Memory layer: the permanent evidence library, its per-topic study ledger,
-- the owner's expert layer and the post-run job queue. Additive, safe to run
-- once via sqlx migrate: new tables only, no existing table changes.
-- Records are permanent and immutable; only the link to the run (and so to a
-- user) is cleared when the run expires. Row-level security is on for every
-- table with no policies (the Data API is off; this is a second lock). The
-- app connects as the owner role and filters in code.

CREATE TABLE incisive.topics (
    id BIGSERIAL PRIMARY KEY,
    label TEXT NOT NULL,
    -- Sorted, normalised intervention-comparator pairs from the era table.
    pairs JSONB NOT NULL,
    embedding vector(768),
    embed_model TEXT,
    -- A merged topic stays as a pointer, never deleted.
    merged_into BIGINT REFERENCES incisive.topics(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE incisive.evidence_records (
    id BIGSERIAL PRIMARY KEY,
    topic_id BIGINT NOT NULL REFERENCES incisive.topics(id),
    -- Per topic: 1, 2, 3...
    version INT NOT NULL,
    -- Not cascade: when the run expires the link is cleared.
    source_run_id UUID REFERENCES incisive.runs(id) ON DELETE SET NULL,
    question TEXT NOT NULL,
    answer JSONB NOT NULL,
    answer_version SMALLINT NOT NULL,
    board JSONB NOT NULL,
    era_table JSONB NOT NULL,
    evidence_through DATE NOT NULL,
    topic_label TEXT NOT NULL,
    -- Outcome names plus the first At a glance line.
    fts_extra TEXT NOT NULL DEFAULT '',
    fts tsvector GENERATED ALWAYS AS (
        to_tsvector('english',
            coalesce(question, '') || ' ' || coalesce(topic_label, '') || ' ' || coalesce(fts_extra, ''))
    ) STORED,
    embedding vector(768),
    embed_model TEXT,
    embedded_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (topic_id, version)
);

CREATE INDEX evidence_records_fts_idx ON incisive.evidence_records USING gin (fts);
CREATE INDEX evidence_records_embedding_idx
    ON incisive.evidence_records USING hnsw (embedding vector_cosine_ops);
CREATE INDEX evidence_records_topic_idx ON incisive.evidence_records (topic_id);
CREATE INDEX evidence_records_through_idx ON incisive.evidence_records (evidence_through);
CREATE INDEX topics_embedding_idx ON incisive.topics USING hnsw (embedding vector_cosine_ops);

-- One row per study per topic; a study found again is counted once.
CREATE TABLE incisive.ledger_studies (
    topic_id BIGINT NOT NULL REFERENCES incisive.topics(id),
    pmid TEXT NOT NULL,
    pub_year INT,
    treatment_start INT,
    treatment_end INT,
    years_assumed BOOLEAN NOT NULL,
    era_id TEXT,
    first_record_id BIGINT NOT NULL REFERENCES incisive.evidence_records(id),
    last_record_id BIGINT NOT NULL REFERENCES incisive.evidence_records(id),
    PRIMARY KEY (topic_id, pmid)
);

-- Per outcome, per era, per record; taken directly from the stored table.
CREATE TABLE incisive.ledger_claims (
    record_id BIGINT NOT NULL REFERENCES incisive.evidence_records(id),
    topic_id BIGINT NOT NULL REFERENCES incisive.topics(id),
    outcome TEXT NOT NULL,
    intervention TEXT NOT NULL,
    comparator TEXT NOT NULL,
    era_id TEXT NOT NULL,
    era_start INT NOT NULL,
    era_end INT NOT NULL,
    estimate NUMERIC,
    lo NUMERIC,
    hi NUMERIC,
    effect_type TEXT NOT NULL,
    n_studies INT NOT NULL,
    -- The trajectory label.
    label TEXT NOT NULL,
    PRIMARY KEY (record_id, outcome, intervention, comparator, era_id)
);

-- The expert layer, append-only: the current verdict is the newest row.
CREATE TABLE incisive.topic_verdicts (
    id BIGSERIAL PRIMARY KEY,
    topic_id BIGINT NOT NULL REFERENCES incisive.topics(id),
    verdict TEXT NOT NULL CHECK (verdict IN ('adopt','selective','not_yet')),
    note TEXT NOT NULL DEFAULT '',
    based_on_record_id BIGINT REFERENCES incisive.evidence_records(id),
    author_user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Owner-reviewed versions, append-only: a newer row for the same part supersedes.
CREATE TABLE incisive.owner_revisions (
    id BIGSERIAL PRIMARY KEY,
    record_id BIGINT NOT NULL REFERENCES incisive.evidence_records(id),
    part TEXT NOT NULL
        CHECK (part IN ('answer_bottom_line','answer_section','opinion','opinion_part')),
    -- Which section or part, e.g. an outcome name or an opinion part number.
    part_key TEXT,
    -- The exact Claude passage being revised, copied at edit time.
    claude_text TEXT NOT NULL,
    owner_text TEXT NOT NULL,
    author TEXT NOT NULL DEFAULT 'owner',
    author_user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Merge/split log, with a payload enough to reverse each change.
CREATE TABLE incisive.topic_changes (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('merge','split','rename','undo')),
    payload JSONB NOT NULL,
    author_user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Post-run work queue, one job per run; goes with the run.
CREATE TABLE incisive.memory_jobs (
    run_id UUID PRIMARY KEY REFERENCES incisive.runs(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','running','done','failed','skipped')),
    attempts INT NOT NULL DEFAULT 0,
    -- A fixed code; never text from the answer.
    last_error_kind TEXT,
    record_id BIGINT REFERENCES incisive.evidence_records(id),
    claimed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX memory_jobs_pending_idx ON incisive.memory_jobs (status) WHERE status = 'pending';

ALTER TABLE incisive.topics ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.evidence_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.ledger_studies ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.ledger_claims ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.topic_verdicts ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.owner_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.topic_changes ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.memory_jobs ENABLE ROW LEVEL SECURITY;
