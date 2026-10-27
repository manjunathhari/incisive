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
| C. Money and records | 4, 5 | ✅ complete (27 Oct) |
| D. Ask pipeline and screens | 6 | ✅ complete (27 Oct) |
| E. Production wiring and final review | 7 + review | ✅ complete (27 Oct) — awaiting your settings and merge go-ahead |

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

## Stage C: Money and records ✅

**Date:** 27 Oct 2026 · **Commits:** `c20cd6e..7021a58` (4 commits) · **GitHub CI:** tests passed on `7021a58` (run 40495376933). The container build for that run was cancelled because a later run superseded it, and the container build is covered by the Stage D run. **Unresolved flake:** two runs with this same code (`4f71b1a`, and `72fb69b` which changed only the build log) failed in the test step. GitHub's logs could not be retrieved from the build environment. CI now reports failing test names directly. Suspects are being fixed in Stage E.

### What was built (plain English)
- **Spending caps (Task 4).** Every question reserves its estimated cost *before* anything is spent, then settles to the real cost afterwards.
  - Money is stored as whole paise. "Today" and "this month" follow the Indian clock (midnight IST).
  - There are three limits: your personal daily limit, a personal monthly limit (surgeons and fellows only), and an overall monthly budget for the whole app.
  - When a limit would be crossed, the user sees a plain message ("You've reached today's Incisive limit (₹X). It resets at midnight IST.").
  - At 80% of any limit, an alert line appears.
- **Tamper-evident activity log (Task 5).** Every step of every request is written to a log where each entry carries a fingerprint of the one before it. If anyone edits or deletes a past entry, the chain shows "broken at #N".
  - It never stores question text, identifiers, or the gate model's own wording. It stores only event types, finding kinds and positions, verdicts, token counts, paise, and a fingerprint of accepted questions.

### Tasks
| Task | Review verdict | Fix rounds | Commits |
|---|---|---|---|
| 4. Spending caps | Approved first time; 1 hardening round | 1 | c20cd6e, 86dd553 |
| 5. Activity log | Approved after 1 fix round | 1 | 4f71b1a, 7021a58 |

### Tests
- **Spending:** 16 tests. They cover:
  - rounding up to the paisa, including float-noise cases
  - an unknown model refused
  - IST day and month rollover at 18:29 and 18:31 UTC
  - the 80% alert, and the overall cap blocking everyone
  - a request settled twice being charged once
  - two simultaneous requests from the same user, and from two different users, racing for the last of a budget: exactly one wins
- **Activity log:** 19 tests. They cover:
  - fingerprints being stable
  - an edited entry and a deleted middle entry both detected
  - 50 simultaneous writes never forking the chain
  - decimals rejected (see below)
  - question text never appearing in stored details

### Review findings (summary)
- **Spending:** no serious issues. The hardening round made a repeated settlement harmless, documented that one request can overrun its estimate slightly, and added the two-user race test.
- **Activity log:**
  - The review caught that a very large decimal number comes back from the database in a different form, which would have made the log *falsely* report tampering. Decimals are now refused outright.
  - The gate's model-written reason is no longer stored, because only the fixed rule wording is safe. The model is also told not to quote the question.

### Decisions made during the stage (rulings)
- **Lock order:** overall budget first, then the user. This prevents two requests from waiting on each other forever. *Cost if wrong:* none.
- **Database permissions:** the log's protection is *detection* (the chain), not prevention. Supabase connects the app as the database owner, and an owner can't be stopped from editing. Deleting the *newest* entries can't be detected from the chain alone. A periodic external copy of the latest fingerprint would close this, and is logged as a follow-up.
- **Fingerprint of accepted questions:** this is a plain SHA-256 hash, as planned. A short, guessable question could be confirmed by someone with database access. The follow-up is to key it with a server secret.
- **A charge is dated when it is settled.** A request that straddles midnight IST counts towards the new day.

### Known limitations / follow-ups
- If the final settlement fails after a model call, the reservation stays open and counts against that user's limit. It has no automatic expiry yet.
- The Activity page re-checks the whole chain on every load. That is fine at this size; a checkpoint is a later optimisation.

---

## Stage D: Ask pipeline and screens ✅

**Date:** 27 Oct 2026 · **Commits:** `cd4f631..9011255` (2 commits, plus a CI reporting improvement `b530a7d`) · **GitHub CI:** run 40499245473 on `9011255`, all checks passed, including the container build with the browser (WASM) build of the new screens and the smoke test

### What was built (plain English)
- **The question box.** You type a question and press Send, and you get one of these results:
  - **Red:** "remove these and resend", listing *what kind* of identifier was found (e.g. "phone number", "name"). The text itself is never echoed back.
  - **Amber:** "Incisive only answers medical and surgical questions", "Could you say a little more…", "I couldn't check this question just now", or "You've reached today's limit".
  - **Green:** "Accepted. The evidence agent arrives in Phase 2." If you're near a limit, the alert line shows too.
- **The order is fixed:** sign-in → identifier filter → spending reservation → medical gate → settle cost → log. Every step is written to the activity log under one request ID.
- **Fail-closed everywhere.** If the log can't be written, the request is refused ("Incisive couldn't record this request"). An answer is never given without a record. If the gate's model has no price set, the question is refused rather than charged at zero.
- **Activity page (owner only).** It shows time (IST), user, event and short detail, with a badge: "Chain verified ✓ (N entries)" or "Chain broken at #N". Others get "not allowed".
- **"Signed in as …"** banner and a `/api/me` endpoint showing today's spend against your limit.
- **Input limits.** Empty questions and questions over 4,000 characters are stopped immediately, at no cost and with no log entry. The box enforces the limit too, and Send is disabled while the box is empty.

### Tasks
| Task | Review verdict | Fix rounds | Commits |
|---|---|---|---|
| 6. Ask pipeline, question box, Activity | Approved after 1 fix round | 1 | cd4f631, 9011255 |

### Tests
- 21 pipeline tests and 1 dedicated logging test. They cover:
  - identifiers blocked before the gate is ever called
  - refusals, unclear, gate failure and over-limit, each audited correctly
  - the accepted path audited in order: gate → charge → accepted
  - **question text never reaching the database or the server logs**
  - the Activity page owner-only, on both routes
  - the developer sign-in shortcut ignored in production
  - log-write failure refusing the request
- Full suite: **152 tests passing**, including when squeezed onto 2 CPUs with 16 tests at once, like GitHub's machines.

### Review findings (summary)
- The pipeline, failure handling, privacy and sign-in were judged sound on first review.
- The one real problem: the "no question text in server logs" test could pass or fail depending on which other tests ran first, because of a quirk in the logging library. Even when it passed, it wasn't checking the pipeline's own messages. It now runs on its own and asserts that the pipeline's messages are present and contain no question text. It was proven to fail if the question is ever logged.
- The implementer also caught a build bug that would have broken the production container: a library feature was enabled only for tests. It was fixed before it reached GitHub.

### Decisions made during the stage (rulings)
- **Spending is reserved before the gate**, because the gate call itself costs money.
- **"52 y/o M, robotic LAR, ICG for leak?"** goes to the model check rather than being approved by keywords. Only "ICG" is on the specific-term list. That costs a fraction of a paisa and is safer.
- **Sign-in rejections are logged** on the ask entry points (reason only, nothing identifying).
- **Empty or over-long questions are not logged:** nothing was decided, and logging them would let anyone signed in write log rows without limit.

### Known limitations / follow-ups
- The question box and Activity page are verified by tests and the container build. The first real browser check happens on the live site after merge.
- Offsets in the red-box data are byte positions (documented); the screen doesn't use them.

### Next stage
**Stage E: Production wiring and final review.** Deploy settings, a final whole-phase review, then merge on your go-ahead.

## Stage E: Production wiring and final review ✅

**Date:** 27 Oct 2026 · **Commits:** `fa6d767..3705df5` · **GitHub CI:** run 40506856319 on `3705df5`. All checks passed: fmt, clippy, all tests with Postgres, the security audit, the container build including the browser (WASM) build, and the container smoke test.

### What was built (plain English)
- **Deploy settings (Task 7).** The live site needs six new settings, listed below.
  - Before building anything, the deploy checks that none are blank, that the model-price list is valid JSON, and that the exchange rate and budget are numbers. If a setting is missing, the live site stays on the old version instead of breaking.
  - The deploy passes the price list through a file, so its commas and quotes can't be mangled.
  - In production, the app refuses to start if the gate model or the main model has no price.
- **GitHub's automatic smoke test** now runs the real container in production mode. It confirms the home page, the question box, the health checks, and that asking without signing in is refused.
- **Instructions** (infra/README.md §10): where each setting comes from, and how to add a colleague. That means adding them as a Google test user, giving them the "IAP-secured Web App User" role, and adding one database row with their role and limits (SQL included).

### Final whole-phase review
A fresh reviewer on the most capable model checked all 37 commits together. Its verdict was "merge after fixes". These were fixed and re-verified:
1. **Hidden characters got past the identifier filter.** A phone number with an invisible zero-width space inside it ("98450​12345"), full-width digits, or look-alike slashes in a date went through to Claude. A first fix banned specific characters. The re-review found more look-alikes, so the rule was reversed:
   - Incisive now accepts **only** plain English text, accented Latin letters, Greek letters and a fixed list of clinical symbols (µ ° ± ≥ ² ₂ ₹ and curly quotes).
   - Everything else is blocked with "please retype without copy-pasted special characters (use plain hyphens, colons and spaces)".
   - Checked against 28 known leak attempts (all blocked) and 22 everyday clinical lines (all pass).
2. **Unreadable gate replies were logged as ₹0** even though the call was billed, so limits could be bypassed. Now:
   - Unreadable replies are charged at their real cost.
   - Timeouts are charged at the estimate.
   - Errors that Anthropic doesn't bill are charged ₹0.
3. **Smaller hardening:**
   - Crash messages can no longer print question text into the server logs.
   - The gate's cost estimate now grows with the question's length.
   - Empty and over-long questions get their own "invalid input" status.
   - Requests sent from other websites are refused before anything else runs.
4. **Intermittent GitHub test failures:** the likely causes were removed. These were a timing-sensitive sign-in test, a test that could run out of database connections, tests that assumed nobody else had written to the log, and tests that could fail if run across midnight IST. GitHub now also prepares the database once before testing.
   - Full suite: **175 tests**, run twice on a fresh database squeezed onto 2 CPUs: all passing.

### Decisions made during the stage (rulings)
- **The GitHub smoke test runs in production mode** with dummy values. That is stronger than the plan's dev mode, because it proves production settings are read correctly.
- **Characters allowed are a fixed list (fail closed).** *Cost:* questions typed in Kannada or Hindi script are blocked for now, as are some pasted symbols: en-dash ranges like "65000–80000", "Grade Ⅲ", and non-breaking spaces next to numbers. The user is told to retype. Incisive is English-only in Phase 1.
- **Gate retries:** if a gate call times out and is retried, only the last attempt is recorded. *Cost:* at most about 30 paise under-counted per question. The follow-up is to stop retrying timeouts on the gate.
- **The same-site check compares the web address but not the port,** when Cloud Run's address has no port. It can't be abused on Cloud Run.

### Follow-ups for later phases
- Patient-identifier filter:
  - a place-name rule (villages, taluks, "R/o Hoskote")
  - "Husband:", "Guardian:" and "Surname:" fields
  - a name written directly before a diagnosis
- Activity log:
  - key the question fingerprint with a server secret
  - an external copy of the latest log fingerprint, to detect deleted newest entries
- Money:
  - reservations left open by a failed settlement have no expiry
  - the gate shouldn't retry timeouts
- Operations: if a deploy's database migration fails, the new version serves errors. Rolling back to Phase 0 isn't possible once the Phase 1 tables exist; roll forward with a fix instead.

## Rulings I made (whole phase)
These are the decisions I took on your behalf during the build. Each is recorded in its stage report, with the reasoning and the cost if it was wrong:
- **Pre-flight:** 8 rulings (top of this log).
- **Stage A:** library backend, temporary `/api/me`, owner limits not overwritten, `APP_ENV` required, sign-in rejections audited later.
- **Stage B:**
  - month-year allowed (**your decision pending**)
  - "s/o" / "d/o" + name always blocks
  - bare "c/o" / "w/o" pass
  - one extra fix past the review cap for privacy leaks
  - the gate's keyword list is best-effort
  - billing words always go to the model
  - configuration faults fail closed
- **Stage C:**
  - lock order
  - tamper *detection* rather than prevention
  - plain SHA-256 question fingerprint
  - decimals rejected in the log
  - the gate's model-written reason never stored
  - charges dated at settlement
- **Stage D:**
  - spend reserved before the gate
  - "ICG for leak" goes to the model
  - empty or over-long questions not logged
  - log failure refuses the request
  - Activity is owner-only
- **Stage E:**
  - smoke test in production mode
  - deploy settings in README §10
  - reject (not clean up) unusual characters, then switch to an allowlist
  - billed gate failures charged
  - gate retry under-count accepted as a follow-up
