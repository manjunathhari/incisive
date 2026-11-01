# Architect — cross-cutting

- [DISCOVERED] Runs: `incisive.runs` (user_id, question, mode, status, cost) → `run_answers` (body JSONB AnswerV3, answer_version) → `run_steps` → `run_temporal` (table_json = TemporalTable, board_json = ExpertBoardV1, topic, evidence_year). All child tables cascade from runs.
- [DISCOVERED] Temporal: pure module `src/temporal/` builds `TemporalTable { eras, studies, rows, outcomes… }` from the stored answer; post-run job behind `TEMPORAL=on|off` (default off).
- [DISCOVERED] Framing produces `Pico { population, intervention, comparison, outcomes, limits }` (`agent/answer.rs`), but AnswerV3 does not store it.
- [DISCOVERED] Library: `/history`, last 100 runs per user, in-memory substring search. No FTS, no vectors (pgvector extension is enabled in 0001 but unused).

## Active features
- memory-layer (Phase 0)
