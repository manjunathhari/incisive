-- Europe PMC records carry a PMC id; keep it with the cached paper.
ALTER TABLE incisive.paper_cache ADD COLUMN pmcid TEXT;
