-- Knowledge graph: concepts, papers, and their per-run links, plus the job
-- queue. Additive, new tables only. Per-run rows go with the run; concepts
-- and papers are shared and stay. Row-level security is on for every table
-- with no policies (the Data API is off; this is a second lock), as in 0013.

CREATE TABLE incisive.graph_concepts (
  id BIGSERIAL PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('disease','treatment','organ','outcome')),
  label TEXT NOT NULL, norm_label TEXT NOT NULL,
  merged_into BIGINT REFERENCES incisive.graph_concepts(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (kind, norm_label),
  CHECK (merged_into IS NULL OR merged_into <> id));
CREATE TABLE incisive.graph_papers (
  pmid TEXT PRIMARY KEY, title TEXT NOT NULL, journal TEXT, year INT,
  doi TEXT, retracted BOOLEAN NOT NULL DEFAULT FALSE,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE incisive.graph_run_concepts (
  run_id UUID NOT NULL REFERENCES incisive.runs(id) ON DELETE CASCADE,
  user_id BIGINT NOT NULL REFERENCES incisive.users(id),
  concept_id BIGINT NOT NULL REFERENCES incisive.graph_concepts(id),
  certainty TEXT, source_row INT,
  PRIMARY KEY (run_id, concept_id));
CREATE TABLE incisive.graph_run_papers (
  run_id UUID NOT NULL REFERENCES incisive.runs(id) ON DELETE CASCADE,
  user_id BIGINT NOT NULL REFERENCES incisive.users(id),
  pmid TEXT NOT NULL REFERENCES incisive.graph_papers(pmid),
  rank TEXT NOT NULL CHECK (rank IN ('rct','meta','prospective','retrospective','case_series','guideline','model','unclassified')),
  rank_label TEXT NOT NULL, includes_non_rct BOOLEAN NOT NULL DEFAULT FALSE,
  design TEXT NOT NULL DEFAULT '', n BIGINT,
  PRIMARY KEY (run_id, pmid));
CREATE TABLE incisive.graph_jobs (
  run_id UUID PRIMARY KEY REFERENCES incisive.runs(id) ON DELETE CASCADE,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','running','done','failed','skipped','cap_blocked')),
  attempts INT NOT NULL DEFAULT 0, last_error_kind TEXT, dropped INT NOT NULL DEFAULT 0,
  cost_paise BIGINT NOT NULL DEFAULT 0, request_id UUID,
  next_try_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), finished_at TIMESTAMPTZ);

CREATE INDEX graph_run_concepts_user_idx ON incisive.graph_run_concepts (user_id);
CREATE INDEX graph_run_concepts_concept_idx ON incisive.graph_run_concepts (concept_id);
CREATE INDEX graph_run_papers_user_idx ON incisive.graph_run_papers (user_id);
CREATE INDEX graph_jobs_next_try_idx ON incisive.graph_jobs (next_try_at)
  WHERE status IN ('pending','cap_blocked');

ALTER TABLE incisive.graph_concepts ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.graph_papers ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.graph_run_concepts ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.graph_run_papers ENABLE ROW LEVEL SECURITY;
ALTER TABLE incisive.graph_jobs ENABLE ROW LEVEL SECURITY;
