-- Prompt caching (Phase 3 Stage 3, §X): the cache tokens of a charge, beside
-- input_tokens and output_tokens. Additive and nullable: older rows and calls
-- without caching keep NULL / 0. Counts only, never content.
ALTER TABLE incisive.spend
    ADD COLUMN cache_creation_input_tokens INT,
    ADD COLUMN cache_read_input_tokens INT;
