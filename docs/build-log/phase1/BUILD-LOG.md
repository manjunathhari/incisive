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
| B. Safety filters | 2, 3 | ✅ complete (27 Oct) |
| C. Money and records | 4, 5 | ⏳ in progress |
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

## Stage B: Safety filters ✅

**Date:** 27 Oct 2026 · **Branch:** `phase1-safety-core` · **Commits:** `2a87358..fcc56d8` (11 commits) · **GitHub CI:** run 40492553405 on `fcc56d8`, all checks passed (fmt, clippy, tests with Postgres, security audit, container build including the browser/WASM build, container smoke test)

### What was built (plain English)
- **Patient-identifier filter (Task 2).** Every question is scanned *on our own server, before anything is sent anywhere*. If it contains something that could identify a patient, it is stopped and the user is told exactly what to remove. Nothing is sent to Claude and the question text is never stored.
  - **Blocked:** UHID/IP/MRN-style numbers, Aadhaar, PAN, Indian phone numbers, email addresses, street addresses and PIN codes, exact calendar dates (DOA/DOD, "12/10/2026"), and names in patient context ("Patient Ramesh Kumar", "NAME: RAMAKKA", "S/O MUNIYAPPA", "W/O Kariyappa, 48 yrs", "Smt. Gowramma").
  - **Allowed, as you decided:** ages, sex, and relative time ("65-year-old male", "POD 3", "6 weeks after surgery"), plus eponyms and clinical shorthand ("Whipple", "Billroth II", "s/o acute appendicitis", "c/o pain abdomen, 10 years", "Couinaud segment 4b").
- **Medical-only gate (Task 3).** After the identifier filter, each question is classified as **medical**, **not medical** or **unclear**.
  - A quick keyword check settles the obvious cases for free. For example, "ICG in robotic low anterior resection — evidence for leak?" is medical and "bitcoin price tomorrow" is not.
  - Everything else goes to a small, cheap Claude model (Haiku, configurable), which returns a strict one-line verdict.
  - Billing, coding, finance, admin and non-medical questions are refused politely. "Unclear" gets one clarifying prompt instead of a refusal.
  - **It fails closed.** If the check can't run (API down, odd reply), the question is *not* answered: "I couldn't check this question just now. Please try again in a moment."

### How to see it / what changes for the user
Nothing visible on the live site yet. These filters are wired into the Ask screen in Stage D and go live when Phase 1 is merged.

### Tasks
| Task | Review verdict | Fix rounds | Commits |
|---|---|---|---|
| 2. Patient-identifier filter | Approved after 5 review rounds plus 1 final scoped fix (cap ruling) | 5 + 1 | 2a87358 … 703e5c2 |
| 3. Medical-only gate | Approved after 3 fix rounds | 3 | bcb4534 … fcc56d8 |

### Tests
- **Identifier filter:** about 190 "must block" rows and 240 "must pass" rows of real-world Indian case-sheet and surgical text, plus about 130 adversarial probes in a regression corpus that every future change must keep passing.
- **Gate:** 21 tests.
  - A rule table of clinical, billing, finance, admin and trick questions.
  - Strict parsing of the model's reply: prose, two objects, unknown labels, extra fields, wrong case and odd code fences are all rejected.
  - Retry and fail-closed behaviour when the API is overloaded (529) or rejects the request (4xx).
  - The request uses the configured gate model with a 150-token limit.
  - Rules run before the model, so there is no API call when a rule decides.
- **Full suite:** green locally against Postgres 16 + pgvector, and on GitHub.

### Review findings (summary)
**Identifier filter.** This was the hardest piece of Phase 1. Indian clinical shorthand collides with identifier patterns: "s/o" means both "son of" and "suggestive of", "c/o" means both "care of" and "complains of", and many surnames end like diagnoses. Five review rounds, each with adversarial probing, pushed in both directions:
- **Leaks fixed:**
  - "son of" names before a header word ("S/O NAGESH GOWDA DIAGNOSIS: …")
  - "56 years of age" after a c/o name
  - upper-case case-sheet headers ("NAME: RAMAKKA", "PATIENT NAME: …")
  - phone numbers next to lab words
  - PIN codes in prose
- **False blocks fixed:**
  - "s/o Sebaceous Cyst", "Intestinal Malrotation", "Signet Ring Cell Carcinoma"
  - Couinaud segments, lab ranges and units, "Lt"/"Late" titles
- **Final approach:** a clinical-word list plus a list of common Indian given names decides the ambiguous cases. A fresh, more capable model did rounds 4–5.

**Gate.** The first version let general words ("surgical", "robotic", "patient") approve a question by keyword alone, so "robotic surgery market size and investment returns" would have skipped the check. It also refused "cryptorchidism" because it contains "crypto". The fixes:
- Only specific clinical terms (procedures, diseases, anatomy, study designs) count toward approval.
- Words match whole, not as fragments.
- Billing, coding, finance, insurance, pay and marketing vocabulary always sends the question to the model check.
- Invisible-character tricks ("Bil­ling") are stripped first.
- A question can't break out of its "treat this as data" wrapper.

### Decisions made during the stage (rulings)
- **Month-year is allowed** ("operated in March 2024"). Your policy blocks *exact* dates. Note: the US HIPAA "Safe Harbor" standard is stricter and would block month-year too. *Cost if wrong:* a month-year of surgery slightly narrows who a patient could be. **Tell me if you want month-year blocked.**
- **"s/o" or "d/o" plus a capitalised name always blocks**, unless what follows is clearly a clinical phrase. A bare "c/o X" or "w/o X" passes when X isn't a known given name, because these usually mean "complains of" or "without". *Cost if wrong:* a bare first name like "W/O MUNIRAJU" can pass.
- **Review cap on Task 2:** after the 5th round, three real leaks remained. Because this is patient privacy, I did not park them. One tightly scoped fix went in, and I verified it myself (26 probes, all correct) instead of running a 6th review round.
- **Gate deviation accepted:** a question with billing or finance wording is never approved by keyword alone; it always goes to the model. *Cost:* a fraction of a paisa per such question.
- **Configuration faults fail closed:** a rejected API request (e.g. a wrong `GATE_MODEL`) shows the user "try again in a moment". The server log records the real cause.
- **The gate's keyword list is best-effort.** It can only *skip* the model for clearly clinical questions; it never refuses a clinical question. Misspellings ("B1lling") are a known residual, caught by the model check and by the medical-only answering agent.

### Known limitations / follow-ups
Identifier filter gaps, all documented:
- a name followed directly by a diagnosis with no comma or label ("S/O MUNIYAPPA HERNIA")
- "Bengaluru 560038 patients" (a place word plus a count)
- relative fields given as a bare label ("Husband: Nagaraju", "Guardian: …"); "Husband Name:" and "Father's Name:" do block
- **places written without a PIN code** ("R/o Hoskote", "Village/Taluk/Dist"), which need a separate place-name rule; proposed as a follow-up
- "Surname: RAO"
- "Sector 2" over-blocks
- when two findings overlap, only the first kind is reported

Gate: the model's one-line reason is shown to the user; Stage D must render it as plain text.

### Next stage
**Stage C: Money and records.** Task 4 is the spending caps, in paise on Asia/Kolkata days and months. Task 5 is the tamper-evident audit log.
