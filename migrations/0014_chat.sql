-- Evidence chat: passages built from stored evidence records, per-run
-- conversations and turns, Q&As the owner approves per topic, and Gemma's
-- unverified prior. Additive: new tables, plus one counter column on
-- evidence_records. Passages, prep state, Q&As and priors are permanent with
-- the record; conversations and turns go with the run. Row-level security is
-- on for every table with no policies (the Data API is off; this is a second
-- lock). The app connects as the owner role and filters in code.

ALTER TABLE incisive.evidence_records ADD COLUMN embed_attempts INT NOT NULL DEFAULT 0;

-- One row per retrievable passage of a record's evidence.
CREATE TABLE incisive.chat_passages (
    id BIGSERIAL PRIMARY KEY,
    record_id BIGINT NOT NULL REFERENCES incisive.evidence_records(id),
    kind TEXT NOT NULL,
    seq INT NOT NULL,
    -- The reference numbers and PMID the passage rests on; a passage can cover
    -- several papers or several arms.
    ref_ns INT[] NOT NULL DEFAULT '{}',
    pmid TEXT,
    comparisons TEXT[] NOT NULL DEFAULT '{}',
    text TEXT NOT NULL,
    -- A belief passage points at the passage that settled it.
    settling_passage_id BIGINT REFERENCES incisive.chat_passages(id),
    fts tsvector GENERATED ALWAYS AS (to_tsvector('english', coalesce(text, ''))) STORED,
    embedding vector(768),
    embed_model TEXT,
    embed_attempts INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (record_id, kind, seq)
);
CREATE INDEX chat_passages_fts_idx ON incisive.chat_passages USING gin (fts);
-- No hnsw index: retrieval scans one record's rows.
CREATE INDEX chat_passages_record_idx ON incisive.chat_passages (record_id);

-- Whether a record's passages were built; a record with no row here is next.
CREATE TABLE incisive.chat_prep (
    record_id BIGINT PRIMARY KEY REFERENCES incisive.evidence_records(id),
    status TEXT NOT NULL CHECK (status IN ('ok','failed')),
    passages INT NOT NULL DEFAULT 0,
    -- A fixed code; never text from the answer.
    error_kind TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Conversations and turns go with the run.
CREATE TABLE incisive.chat_conversations (
    id BIGSERIAL PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES incisive.runs(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    mode TEXT NOT NULL CHECK (mode IN ('clinical','plain')),
    is_eval BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX chat_conversations_run_idx ON incisive.chat_conversations (run_id, user_id);

CREATE TABLE incisive.chat_turns (
    id BIGSERIAL PRIMARY KEY,
    conversation_id BIGINT NOT NULL REFERENCES incisive.chat_conversations(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    seq INT NOT NULL,
    question TEXT NOT NULL,
    reply TEXT NOT NULL DEFAULT '',
    mode TEXT NOT NULL CHECK (mode IN ('clinical','plain')),
    status TEXT NOT NULL
        CHECK (status IN ('pending','done','partial','not_covered','unavailable')),
    passage_ids BIGINT[] NOT NULL DEFAULT '{}',
    checks JSONB,
    safety_line BOOLEAN NOT NULL DEFAULT FALSE,
    personal BOOLEAN NOT NULL DEFAULT FALSE,
    feedback SMALLINT CHECK (feedback IN (-1, 1)),
    suggested BOOLEAN NOT NULL DEFAULT FALSE,
    latency_ms INT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (conversation_id, seq)
);
-- One reply in flight per user, across instances.
CREATE UNIQUE INDEX chat_turns_one_pending_idx
    ON incisive.chat_turns (user_id) WHERE status = 'pending';

-- Q&As suggested from turns and approved by the owner, per topic.
CREATE TABLE incisive.topic_qas (
    id BIGSERIAL PRIMARY KEY,
    topic_id BIGINT NOT NULL REFERENCES incisive.topics(id),
    source_turn_id BIGINT REFERENCES incisive.chat_turns(id) ON DELETE SET NULL,
    question TEXT NOT NULL,
    gemma_reply TEXT NOT NULL,
    owner_text TEXT,
    status TEXT NOT NULL DEFAULT 'suggested'
        CHECK (status IN ('suggested','approved','rejected')),
    author_user_id BIGINT NOT NULL REFERENCES incisive.users(id),
    fts tsvector GENERATED ALWAYS AS (
        to_tsvector('english', coalesce(question, '') || ' ' || coalesce(owner_text, ''))
    ) STORED,
    embedding vector(768),
    embed_model TEXT,
    embed_attempts INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX topic_qas_topic_idx ON incisive.topic_qas (topic_id);
CREATE INDEX topic_qas_fts_idx ON incisive.topic_qas USING gin (fts);

-- Gemma's expected answer, labelled unverified, and its reconciliation.
CREATE TABLE incisive.chat_priors (
    id BIGSERIAL PRIMARY KEY,
    run_id UUID REFERENCES incisive.runs(id) ON DELETE SET NULL,
    record_id BIGINT REFERENCES incisive.evidence_records(id),
    status TEXT NOT NULL
        CHECK (status IN ('writing','written','reconciling','reconciled','failed')),
    prior JSONB,
    reconciliation JSONB,
    scorecard JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (run_id)
);

ALTER TABLE incisive.chat_passages ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.chat_prep ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.chat_conversations ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.chat_turns ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.topic_qas ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.chat_priors ENABLE ROW LEVEL SECURITY;
