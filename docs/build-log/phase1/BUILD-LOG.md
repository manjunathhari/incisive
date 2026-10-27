# Incisive · Phase 1 (Safety Core) · Build Log

Running record of the Phase 1 build. Updated after every task and every stage.

- **Plan:** `docs/superpowers/plans/2026-10-27-incisive-phase1-safety-core.md`
- **Spec:** `docs/superpowers/specs/surgeon-evidence-sandbox-design.md`
- **Branch:** `phase1-safety-core` (from `main` @ c6aa01b)
- **Method:** subagent-driven: a fresh implementer per task, an independent reviewer per task, fix rounds until clean, and a whole-phase review at the end.
- **Started:** 27 Oct 2026

## Progress

| Stage | Tasks | Status |
|---|---|---|
| A. Identity | 1 | ✅ complete (27 Oct) |
| B. Safety filters | 2, 3 | ⏳ in progress |
| C. Money and records | 4, 5 | ⬜ not started |
| D. Ask pipeline and screens | 6 | ⬜ not started |
| E. Production wiring and final review | 7 + review | ⬜ not started |

## Decisions made before the build (pre-flight rulings)

Each was checked against the plan for conflicts before Task 1. Format: what · why · cost if wrong.

1. **Owner setup runs after migrations.** `ensure_owner` is called inside the background migration task, only once migrations have succeeded. The `users` table doesn't exist until then. *Cost if wrong:* none at runtime; at worst the owner row appears a few seconds after startup.
2. **`incisive-app` gains `claude-client` as a server-only dependency** (Task 3), so the gate can call Claude. The browser bundle never includes it. *Cost if wrong:* none.
3. **Task 4 also provides `totals(pool, user_id) -> Totals { today_paise, month_paise }`**, which Task 6's `/api/me` needs but the plan didn't define. *Cost if wrong:* trivial.
4. **Shared dependencies** (`uuid`, `chrono`, `chrono-tz`, `serde`) are added by whichever task needs them first, and later tasks reuse them. *Cost if wrong:* none.
5. **The audit log's protection is the hash chain, not database permissions.** Supabase connects the app as the database owner, and a `REVOKE` can't restrict the owner. So tampering is *detected* by `verify_chain` rather than *prevented*, and this is documented. A separate restricted database role can come later. *Cost if wrong:* an attacker with the database password could still edit rows, but the edit would show up as a broken chain.
6. **Screens use Leptos server functions** that call the same pipeline function as `POST /api/ask`. The JSON routes stay for tests and future use. *Cost if wrong:* small refactor.
7. **The browser (WASM) build is verified on GitHub**, by the container build in CI after each stage. The WASM toolchain can't be downloaded in this build environment. *Cost if wrong:* a browser-only compile error would surface at the stage's CI run instead of during the task.
8. **Database tests run against a local Postgres 16 + pgvector** started for this build. The same tests run on GitHub against the `pgvector/pgvector:pg16` service. *Cost if wrong:* none.

---

<!-- Stage reports are appended below, newest last. -->

## Stage A: Identity ✅

**Date:** 27 Oct 2026 · **Branch:** `phase1-safety-core` · **Commits:** `0c03fbc..72063b6` (2 commits) · **GitHub CI:** run 40479401621, all checks passed (tests, lint, security audit, container build including the browser/WASM build, container smoke test)

### What was built (plain English)
- Incisive now **checks who you are on every request**. Google's sign-in gate attaches a signed note to each request. The app verifies Google's signature, the issuer, which app the note was issued for, and its expiry. Then it looks you up in a new `users` table that holds your role and spending limits.
- **You are set up automatically as the owner** (₹1,500/day, no personal monthly cap) once the database is ready. Later changes to your limits survive restarts.
- A Google account that the gate lets in but that hasn't been added to Incisive gets a clear **"not set up yet"** message.
- **Developer mode is impossible in production.** The app now refuses to start unless its environment is explicitly set to `dev` or `prod`.
- If Google's signing keys can't be fetched (for example, just after the server wakes up), users see **"Sign-in could not be checked just now"** instead of a misleading "not signed in", and the app retries after 10 seconds.

### How to see it / what changes for the user
Nothing visible yet on the live site. Stage A is merged into `main` (and deployed) at the end of Phase 1. A test endpoint `/api/me` returns your name and role.

### Tasks
| Task | Implementer | Review verdict | Fix rounds | Commits |
|---|---|---|---|---|
| 1. Identity from Google sign-in | Done | Approved after 1 fix round (re-review: all findings addressed) | 1 | 666bd87, 72063b6 |

### Tests
- **20 identity tests**, all using real signatures with throwaway test keys:
  - valid sign-in
  - rejections: wrong audience, expired, wrong issuer, unknown key ID, right key ID but wrong key, `alg: none`, HS256 key-confusion attack, garbage token, missing header, unverified email header ignored
  - unknown user gets 403; inactive user gets 403; active user gets through
  - developer header works only in dev
  - owner setup is idempotent
  - key server unreachable gives 503; empty key cache retries after 10 s
- **Config tests:** 8, including "missing APP_ENV is an error".
- **Full suite:** all passing locally against Postgres 16 + pgvector, and on GitHub.

### Review findings
Reviewed by an independent reviewer on the most capable model, because this is the security layer. It confirmed that sign-in verification is correct: only Google's ES256 keys are accepted, the issuer, audience and expiry are enforced, keys are looked up safely, and production never trusts unsigned headers.

Fixed in round 1:
1. **Important:** the CI container check would have failed, because two new required settings weren't passed to it. Fixed.
2. **Important:** a missing environment setting silently meant developer mode, which would bypass sign-in. `APP_ENV` is now required, and dev mode logs a warning.
3. **Promoted from minor:** a single failed key download after a cold start could lock everyone out for 5 minutes and show "not signed in". Now a 10-second retry and a 503 message.

Deferred minor items (logged for the final review): key-cache lock held during the download; no periodic refresh of Google's keys; one test checks only the first layer of the algorithm defence; a rare non-English-letter email case; an unused error variant; a note for local runs (APP_ENV now required).

### Decisions made during the stage (rulings)
- **Signature library backend:** uses `aws_lc_rs`, because the alternative backend pulls in a crate with a security advisory. *Cost if wrong:* slightly heavier build.
- **Temporary `/api/me` endpoint:** Stage D replaces it with the full version. *Cost if wrong:* small refactor.
- **Owner limits:** owner setup doesn't overwrite limits you later change. *Cost if wrong:* none.
- **Sign-in rejections in the audit log:** these are recorded from Stage D, once the audit log exists (Stage C). *Cost if wrong:* none.
- **CI container check fixed now:** rather than waiting for Stage E, so every stage stays green. *Cost if wrong:* none.
- **`APP_ENV` required:** overrides the Phase 0 "default to dev" behaviour. *Cost if wrong:* local runs must set `APP_ENV=dev`.
- **Production deploy settings** (`OWNER_EMAIL`, `IAP_AUDIENCE`): added in Stage E. The branch isn't deployed before then. *Cost if wrong:* none, unless merged early.

### Known limitations / follow-ups
- The live deploy needs two new settings, `OWNER_EMAIL` and `IAP_AUDIENCE`. You'll be asked for them in Stage E.

### Next stage
**Stage B: Safety filters.** Task 2 is the patient-identifier filter, which runs locally; Task 3 is the medical-only gate.
