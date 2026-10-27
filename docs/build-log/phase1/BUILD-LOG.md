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
| A. Identity | 1 | ⏳ in progress |
| B. Safety filters | 2, 3 | ⬜ not started |
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
