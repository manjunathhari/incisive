-- Phase 3: the answer format of each stored answer. Phase 2 answers (every
-- row before this migration) are version 2; the evidence team's AnswerV3 is
-- version 3. Additive: retention and delete cascade from runs as before.
ALTER TABLE incisive.run_answers
    ADD COLUMN answer_version SMALLINT NOT NULL DEFAULT 2;
