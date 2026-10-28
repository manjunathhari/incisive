# Phase 2 final fix wave report (final-review-1)

Base: HEAD 344d641 (which includes the Task 14 and 15 work). Branch: phase2-evidence-agent.

| Commit | Subject |
|---|---|
| 7f5cf05 | fix: final review wave (service failures, time limits, orphaned reservations) |
| 3299669 | docs/deploy: CPU throttling truth, rollback note, agent model price pre-check |

M-10 (BUILD-LOG.md) was not touched, as instructed.

## 1. I-1: an outage no longer reads as "No relevant evidence found"

- **Counting failures.**
  - `screen::Screened` gains `failed_batches`. It counts batches whose call failed (`Llm`/`Parse`) or whose reply held no verdict list. Both failure sites count it; `OverBudget` and `Cancelled` don't.
  - Screen's Note now gives both counts: "Screening failed for N records (K batches); they were not assessed".
  - `extract::Extraction` gains `failed`. It is Extract's existing count of `Llm`/`Parse` errors and non-object replies. Extract already wrote the Note "Extraction failed for N papers; they were not used".
- **Pipeline** (`gather`):
  - If nothing was included and `failed_batches > 0`, or nothing was extracted and `failed > 0`, it sets `service_failed`.
  - `assemble` copies it to `trail.service_failed` (`serde(default)`) and counts it as degraded, so the run ends **partial**.
- **Answer.** New `answer::SERVICE_FAILED`: "The search could not be completed because the AI service failed; this is not a finding of no evidence." `empty_bottom_line` order:
  1. no claims verified
  2. Write refused
  3. **service failed**
  4. stopped early
  5. NO_EVIDENCE
- **Interpretation, for the controller.** "No paper assessed or extracted because of failures" is read as "the step's result is empty *and* some of its calls failed". If 1 of 2 batches failed and the other excluded everything, the empty result can't be called a finding either, so it is also a service failure. When failures sit beside a non-empty result, only the count Note is added and the status is unchanged, as instructed.
- **Tests** (tests/agent.rs):
  - `all_screen_batches_failing_is_a_service_failure_not_no_evidence`: Partial, `service_failed`, the SERVICE_FAILED bottom line, no NO_EVIDENCE, and a Note saying "(2 batches)".
  - `all_extract_calls_failing_is_a_service_failure_not_no_evidence`: Partial, 2 included, no claims or references, SERVICE_FAILED, and the Extract failure Note.
  - `a_failed_batch_beside_successful_ones_only_adds_a_note`: Done, 3 claims, not `service_failed`, and the Note saying "(1 batches)".

## 2. I-2: README §11, docs only

The "CPU always on" section no longer says polling keeps the run moving. It now says plainly:
- each poll gives only a short burst of CPU, and each Claude call needs a dozen or so database round trips;
- under throttling, Phase 2 is slow even with the page open, and Focused runs may stop early (partial, "time limit") or fail at the hard stop;
- with the page closed or idle, a run stalls and ends `interrupted` after 90 s;
- a stalled worker can hold the spending lock and delay other users;
- **`CPU_ALWAYS=true` is recommended for Phase 2.**

The deploy default is unchanged (`false`); the owner decides.

## 3. M-1: `time_limit_s` now drives wrap-up and the hard stop

- `runs::budget::{wrap_up_after, hard_stop_after}` are `const fn`s: 60% of `time_limit_s`, and `time_limit_s` + 60 s.
- `RunBudget::with_time_limit(time_limit_s)` sets both, and `RunBudget::hard_stop()` exposes the result. Without it, the 300 s defaults apply, so `RunBudget::new` stays 180 s / 360 s.
- The worker builds every budget with `.with_time_limit(state.runs.for_mode(mode).time_limit_s)` and times the pipeline out at `ctx.budget.hard_stop()`.
- The goldset runner's wait deadline uses `hard_stop_after(...)` for the run's mode, plus 60 s.
- The unused `HARD_STOP_AFTER` constant is removed.
- `time_limit_s > 0` was already enforced by `DepthSettings::validate`, and `config.rs` already tests `{"focused": {"time_limit_s": 0}}`.
- README: the `time_limit_s` row says what it controls.
- **Tests:**
  - `time_limits_follow_time_limit_s` (agent): defaults 180/360 (both edges), a 100 s limit giving 60/160 (both edges), and `hard_stop()`.
  - `worker_uses_the_modes_time_limit` (runs): with RUN_LIMITS `time_limit_s` 120, the worker's budget reports a 180 s hard stop.

## 4. M-2: orphaned run reservations are released

- New `store::orphaned_run_reservations`: `kind = 'reservation' AND purpose = 'run'`, made more than 10 minutes ago (`LATE_CHARGE_GRACE_MIN`), with no `runs` row for `request_id`.
- `recover_stale_runs` (at startup and every minute) releases each one with `spend::release`, under the usual locks, and logs only the id.
- Gate reservations (`purpose = 'gate'`) are never touched.
- Test `recovery_releases_reservations_whose_run_is_gone` (exclusive pool). It checks:
  - an orphan older than 15 minutes is released;
  - an orphan just made is kept;
  - an old reservation whose run still exists is kept;
  - an old gate reservation is kept;
  - the user's `today_paise` then counts only the gate reservation.

## 5. M-3 and M-4: documented, no behaviour change

- `spend` module docs gain a "Run overshoots (Phase 2)" section:
  - M-4: deleting an active run releases the whole reservation while calls are in flight, a bounded overshoot. On another instance, at most one more call before the next heartbeat. The freed slot is reusable at once.
  - M-3: unbooked spend leaves the cap accounting once the reservation is released, until it is reconciled by hand from the error log.
- Pointer notes are on `RunBudget::count_unbooked` and `runs::api::delete_one`.
- I judged the M-3 code change not trivial. Keeping `held + unbooked` would be released anyway by the last late call (`release_if_finished`) or by the 10-minute sweep, so making it stick needs more machinery.

## 6. M-5: rollback note

New subsection "Rolling back Phase 2 (prefer rolling forward)" at the end of infra/README.md §11. It covers:
- **Readiness stays down.** Phase 1's sqlx `migrate()` refuses a database with migrations it doesn't know (0005–0009) and retries every 30 s for good, so `/readyz` is 503. Requests are still served, because no startup probe is configured and Cloud Run's default checks only the port.
- **Don't delete `_sqlx_migrations` rows.** Rolling forward would then re-create tables and fail.
- **False alarms.** The Activity page errors and the chain check reports **Broken**, because Phase 1 can't decode Phase 2 audit events. Both are correct again after rolling forward.
- **Release SQL** for run reservations held at rollback: `DELETE FROM incisive.spend WHERE kind = 'reservation' AND purpose = 'run';`, in the Supabase SQL editor or psql. Charges stand. After rolling forward, recovery marks those runs interrupted.

The readiness point goes beyond the review; I found it while checking the rollback path.

## 7. M-6: deploy pre-check for agent model prices

- New block in "Check required repository variables", using Python from `os.environ`, after the key-name check:
  - for each step, it takes the `AGENT_MODELS` value, or the default (mirroring `DEFAULT_MODEL` / `DEFAULT_SCREEN_MODEL` in llm.rs, with a comment saying so);
  - it requires a non-empty name with no whitespace, the same rule as the app;
  - it requires an entry in `MODEL_PRICES`, which must be an object.
- The README notes the check.
- I exercised the step with real env sets:
  - defaults priced → pass
  - a priced override → pass
  - Haiku default unpriced → fails naming `claude-haiku-5-5 (agent step screen)`
  - an unpriced configured model → fail
  - a name with a space → fail
  - non-object `MODEL_PRICES` → fail

## 8. M-7: run page header

- `run_view`'s `<h2>` is the stored Answer's full `question` once there is one, and the 80-character title before that. It is a text node, so Leptos escapes it.
- Test `ssr_run_page_header_is_the_full_question`: a question over 80 characters containing `<i>…</i>` appears escaped and complete inside `<h2>`, and the raw markup appears nowhere.

## 9. M-8: PubMed pacing is per instance

- Doc comment on `lit::pubmed::Pacer`.
- README NCBI subsection: per-instance pacing means up to 3× NCBI's limit with `--max-instances=3`, but only under concurrent load across instances; 429s are retried; the key raises the limit.

## 10. M-9: migration 0009

`migrations/0009_spend_reservation_request_idx.sql`:

```sql
CREATE INDEX IF NOT EXISTS spend_reservation_request_idx ON incisive.spend (request_id) WHERE kind = 'reservation';
```

- **Predicates checked first.** Every reservation lookup filters `request_id = $1 [AND user_id = $2] AND kind = 'reservation'`: `charge_call`, `release`, `release_keeping`, `reservation_owner`. The recovery join uses `request_id`/`user_id`/`kind`, and the orphan sweep `kind` (plus `purpose` and `at`) over reservation rows only.
- It is a partial index, like the existing charge index, and is not UNIQUE.
- **Locking** (stated in the migration): a plain build inside sqlx's migration transaction takes a SHARE lock on `incisive.spend` and blocks writes briefly. The table is small, so well under a second is acceptable. CONCURRENTLY can't run in a transaction.
- No test pins the migration list.

## Gates

```
cargo fmt --all --check                                     -> clean
cargo clippy --workspace --all-targets -- -D warnings       -> Finished, no warnings
cargo clippy -p incisive-app --no-default-features --features hydrate --lib -- -D warnings -> Finished (incisive-app rechecked)
TEST_DATABASE_URL=… cargo test --workspace --no-fail-fast   -> rc=0, 432 passed, 0 failed, no stray output
YAML (PyYAML safe_load) + bash -n on every run: script:
  YAML OK: .github/workflows/deploy.yml (5 run scripts, bash -n OK)
  YAML OK: .github/workflows/ci.yml (8 run scripts, bash -n OK)
```

## Files changed

- crates/incisive-app/src/agent/{answer, extract, pipeline, screen}.rs
- crates/incisive-app/src/app.rs
- crates/incisive-app/src/bin/goldset.rs
- crates/incisive-app/src/lit/pubmed.rs
- crates/incisive-app/src/runs/{api, budget, store, worker}.rs
- crates/incisive-app/src/spend.rs
- crates/incisive-app/tests/{agent, runs}.rs
- migrations/0009_spend_reservation_request_idx.sql
- .github/workflows/deploy.yml
- infra/README.md

## Concerns

1. **I-1 interpretation** (§1): any empty Screen or Extract result that coincides with failures is treated as a service failure. Failures beside a non-empty result add only a Note and keep the status `done`.
2. **Phase 1 rollback readiness** (§6): `/readyz` stays 503 on a Phase 1 image, though requests are served. If a startup or readiness probe on `/readyz` is ever added, a rollback to Phase 1 would not come up at all.
3. **M-3 is only documented.** Unrecorded spend still leaves the cap totals until it is reconciled by hand.
4. **I-2** remains the owner's decision. Under the default throttling, Focused runs are expected to degrade.
5. **The M-6 defaults are duplicated** in deploy.yml. If the code defaults change, that list must change too (the comment says so).

## Final review 2 minors

Base: HEAD 8badbce. One commit, "fix: final review 2 minors (empty screen replies, goldset service note, docs)".

- **N-1** (`agent/screen.rs`): a Screen batch whose reply holds a verdict list but no usable verdict (a non-null `include` for an id in the batch) now counts as a failed batch: `failed += n; failed_batches += 1`. This covers an empty list, every `include` null, and only out-of-batch ids. Such records are counted once, through `failed`. In batches with at least one usable verdict, records without one are still tallied "not assessed" one by one. `Screened::failed_batches` docs updated. A batch beside successful ones still only adds a Note.
- **N-2** (`bin/goldset.rs`): `answer_notes` adds "service failed" when `trail.service_failed`, placed after "write refused". New unit test `answer_notes_names_a_service_failure`.
- **N-3** (`infra/README.md`): rollback note now says "`/ready` (Cloud Run; `/readyz` locally) answers 503".
- **N-4** (`spend.rs`): "may start one more call" became "may start further calls"; the "bounded by the deleted run's limit" statement is kept.
- **N-5** (`app.rs`): the run-page header is `<h2 class="run-question">`, with new CSS `.run-question { overflow-wrap:anywhere; font-size:1.2rem; }` in the app's inline stylesheet. The existing SSR test `ssr_run_page_header_is_the_full_question` (tests/runs.rs) looked for a bare `<h2>`; it now looks for the classed tag.

**Tests added** (tests/agent.rs), each with Frame and Search succeeding and a single screen reply, the later batch failing: Partial, `service_failed`, SERVICE_FAILED bottom line, no NO_EVIDENCE, a "Screening failed for" Note.
- `an_empty_verdict_list_is_a_service_failure_not_no_evidence` (`[]`)
- `a_verdict_list_with_only_null_includes_is_a_service_failure`
- `a_verdict_list_with_only_out_of_batch_ids_is_a_service_failure`

**Gates**
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: Finished, no warnings.
- Hydrate clippy: `cargo clippy -p incisive-app --lib --no-default-features --features hydrate -- -D warnings` (native, as in earlier tasks; the wasm32 target is not installed): Finished, no warnings.
- `cargo test --workspace --no-fail-fast` with TEST_DATABASE_URL: all pass (437 tests; agent 105, runs 67, goldset unit 6). The first run had one failure (the SSR h2 test above), fixed and rerun.
