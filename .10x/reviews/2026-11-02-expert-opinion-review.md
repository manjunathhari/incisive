# Review: Temporal Expert Opinion (T1–T4) on main @ 7ff75a0 — 2 Nov 2026

Method: two independent read-only reviewers (UI T4; engine T1–T3), findings re-verified in code; full suite built and run against local Postgres 16 + pgvector: **830 passed, 0 failed, 5 ignored** (DB tests executed, none skipped).

## Clean
- Run isolation: the job never changes a run's status, answer or cost; only one bounded insert runs inline after the run's finish.
- Money: every hold released on every path (errors, panics, stale rows, leak sweep); cap check under locks; no double charge.
- Scoping: board reachable only through `get_for_user` (run id AND user id); other users get 404.
- No raw HTML from model text; no question/answer text in logs.
- Memory-layer interfaces (ExpertBoardV1, run_temporal, TemporalTable, finish/maybe_start) unchanged by T4.

## Findings (verified)
| # | Sev | Where | Defect | Scenario |
|---|---|---|---|---|
| 1 | Med | app.rs:733–741, 836–841 | Run page stops polling the moment the run ends; the pending temporal row is written just after | Watching a Full run finish → Expert opinion option never appears until manual reload |
| 2 | Med | expert_page.rs:405–421 | Cost filter treats unit `inr` as rupees | A coagulation outcome in INR (International Normalised Ratio) is hidden as a "cost" |
| 3 | Low-Med | expert_page.rs:405–421 + temporal/effects.rs:129 | Currency symbols normalise to `unit`/`us`, so `$ £ € ₹` never match; substring match also hits e.g. `decades` (cad) | "Operating room expense" in `$` is drawn; a unit containing "cad"/"eur" is hidden |
| 4 | Low | runs/temporal.rs:336–350 | Panic handler records cost 0 | Panic after the Opus call is charged → row + audit say ₹0 while spend ledger has the real charge |
| 5 | Low | temporal/maths.rs:65 + mod.rs:1264 | NaN mapped to 0 becomes p(favours)=0 | Degenerate CI → "robust against" instead of undecided; use 0.5/skip |
| 6 | Low | runs/temporal.rs:385–408 | Audit can disagree with the row (two final events; fallback status not reflected) | Stale-recovered row also audited as done |
| 7 | Low | temporal/maths.rs:74, 273 | Fixed predictive SE 0.15 added for difference measures too | MD in days with SE 0.05 → predictive range tripled |
| 8 | Low | temporal/mod.rs:1398, 1463 | DynamicEra/Predictive natural-scale values not passed through `finite` | Absurd ratio → ∞ → null in JSON → board page can't read the table |
| 9 | Low | runs/temporal.rs:252 / 0012 | Leak sweep anti-join on unindexed `request_id`; topic/year indexes unused today | Grows slowly; not urgent (memory layer will use topic) |
| 10 | Low | runs/worker.rs:381–391 | Job can start for a run another instance just marked interrupted | Opinion row on an interrupted run |
| 11 | Low | tests/temporal_ui.rs | Comment claims the answer wrapper is asserted hidden; it isn't | Misleading test comment |

Strengths: pre-hydration CSS fallback for #expert; no-row page byte-checked against pre-T4 baseline; pure rules module tested on three fixtures; flag-never-delete warning chips.
