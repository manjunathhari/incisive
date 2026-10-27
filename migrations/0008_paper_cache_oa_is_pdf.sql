-- Whether the cached open-access link is a PDF (not parsed in Phase 2).
ALTER TABLE incisive.paper_cache ADD COLUMN oa_is_pdf BOOLEAN NOT NULL DEFAULT FALSE;
