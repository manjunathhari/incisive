-- Liveness of a run's worker. Set when the run starts and refreshed while it
-- works (at least every 30 s), always from the database clock. Recovery and
-- self-heal treat a run as abandoned when it is `running` with a heartbeat
-- older than 90 s, or `queued` for over 90 s with no heartbeat.
ALTER TABLE incisive.runs ADD COLUMN heartbeat_at TIMESTAMPTZ NULL;
