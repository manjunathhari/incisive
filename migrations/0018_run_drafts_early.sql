-- Stage S1c: a third draft part, `early`: the finished, checked sections of a
-- Quick answer written in four parts (QUICK_PARALLEL=on), shown before the
-- last part is done. Additive: only the part CHECK is widened. Like the other
-- drafts it is reply text, read only through the run's owner scoping, deleted
-- when the final is saved or the run ends any other way, removed by recovery
-- when stale, and never read by the engine, the memory layer, the graph or
-- the chat.
--
-- 0016 declared the CHECK inline, so Postgres named it run_drafts_part_check.
-- Any CHECK on `part` is dropped by its real name, whatever it is, then the
-- widened one is added under that name.
DO $$
DECLARE c text;
BEGIN
    FOR c IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'incisive.run_drafts'::regclass
          AND contype = 'c'
          AND pg_get_constraintdef(oid) LIKE '%part%'
    LOOP
        EXECUTE format('ALTER TABLE incisive.run_drafts DROP CONSTRAINT %I', c);
    END LOOP;
END $$;

ALTER TABLE incisive.run_drafts
    ADD CONSTRAINT run_drafts_part_check CHECK (part IN ('answer','opinion','early'));
