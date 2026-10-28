# Incisive · Phase 2 (Evidence Agent) · Build Log

A running record of the Phase 2 build, updated after every stage.

- **Spec:** `docs/superpowers/specs/2026-10-27-incisive-phase2-evidence-agent-design.md`
- **Plan:** `docs/superpowers/plans/2026-10-28-incisive-phase2-evidence-agent.md`
- **Branch:** `phase2-evidence-agent`
- **Method:** subagent-driven. Each task gets an implementer, then an independent reviewer, then fix rounds. A whole-phase review comes at the end.
- **Started:** 27 Oct 2026 (night)

## Progress
| Stage | Tasks | Status |
|---|---|---|
| A. Runs and history | 1–4 | ✅ complete (28 Oct) |
| B. Literature tools | 5–7 | ✅ complete (28 Oct) |
| C. Finding the evidence | 8–11 | ⏳ in progress |
| D. Writing and checking | 12–13 | ⬜ not started |
| E. Quick, production, proof | 14–15 + review | ⬜ not started |

## Decisions made before the build
1. **Test data for the literature sources is recorded from the real services.** You allowed PubMed, Europe PMC and Unpaywall in Settings (23:05 IST), so the saved sample responses the tests use come from the live services. Europe PMC was briefly down on its own side when I checked; if it stays down, I will write its sample by hand from the documented format and replace it later.
2. **The activity log keeps recording "Accepted"** when the gate approves a question, and then adds "Run started". *Cost:* one extra log row per question.
3. **This log** lives in the repo and is mirrored to the Project as *incisive-phase2-build-log*.

---
<!-- Stage reports below, newest last. -->

## Stage A: Runs and history ✅

**Date:** 27–28 Oct 2026 · **Commits:** `7fa8505..ed88396` · **GitHub CI:** run 40560436621 on `ed88396`, all checks passed (tests with Postgres, lint, security audit, container build including the browser (WASM) build, smoke test)

### What was built (plain English)
- **Questions become "runs" that work in the background.**
  - Once a question passes the Phase 1 checks (sign-in, identifier filter, medical gate), it is saved as a run and a background worker takes it from there.
  - You can close the page or lock your phone and the run carries on.
  - The steps it is working through appear as a live list on the run's page, which refreshes every 2 seconds.
  - The real evidence steps arrive in Stages B–D. For now each run walks through placeholder steps and ends with a dummy answer, which proves the plumbing end to end.
- **My questions.** This is your private history, newest first. You can open any past run, delete it, or "Delete all my history" (with an on-page confirm). Nobody else can see or delete your runs, including the owner. Runs older than 12 months are deleted automatically each day.
- **Money per run.**
  - Each run sets aside its full limit when it starts (₹40 for Focused).
  - Every AI call made during the run is charged against that, and whatever is left is handed back at the end.
  - A retry can never charge the same call twice.
  - Calls running in parallel can't together overshoot the limit; each must claim its share before it starts.
- **One question at a time per person.** A second Send while one is running gets "You already have a question running."
- **Robust to restarts and to more than one server.**
  - Each running question checks in every 20 seconds.
  - A question that hasn't checked in for 90 seconds is marked "Interrupted — please re-run", and the budget it set aside is released.
  - The cleanup that does this runs at startup and every minute after that.
  - If a question is stuck, your next Send clears it automatically, so nobody stays locked out.
  - Closing the tab at the exact moment a question starts can no longer leave it half-created.
- **Deleting a question while it's running** stops it, keeps any charges already made, and releases the rest of its budget.

### How to see it
Nothing is live yet. Stage A goes live when Phase 2 is merged after Stage E.

### Tasks
| Task | Review verdict | Fix rounds | Commits |
|---|---|---|---|
| 1. Run tables and history store | Approved first time | 0 | cf1efff |
| 2. Per-run budget and charging | Approved after 1 fix round | 1 | 18e8da0, ffc5487 |
| 3. Background worker and recovery | Approved after 1 fix round | 1 | 587b8c0, 5ca515f |
| 4. Run pages, My questions, deletion | Approved first time | 0 | ed88396 |

### Tests
About 50 new run tests and 17 spending tests, all running against a real Postgres database. They cover:
- privacy between users
- one active run per user
- charging and releasing, including the "booked but not recorded, then retried" case
- 32 parallel calls competing for a budget that fits 10: exactly 10 get through
- server-restart recovery and check-in-based cleanup
- a run left half-started when the request is dropped mid-start
- a panic inside a run
- deleting while a run is in progress
- the 12-month cleanup

The full suite passes, including on a fresh database squeezed onto 2 CPUs.

### Review findings (summary)
- **Budget (Task 2):**
  - A retried charge could have been booked twice. Each AI call now carries a fixed ID, so a retry is a no-op.
  - Parallel calls could each pass the "can we afford it?" check before any was charged. Each call now claims its share up front.
- **Worker (Task 3):**
  - Two situations could lock someone out with a false "already running": a tab closed at the wrong instant, and a second server copy cancelling the first copy's live questions at its startup.
  - Fixed with an independent check-in every 20 seconds, cleanup based on missed check-ins, start-up that always completes or is fully undone, and automatic clearing of stuck questions.
- **Tasks 1 and 4** were approved first time. The reviewer confirmed nobody can read or delete another person's questions through any route.

### Decisions made during the stage (rulings)
- **A finished run's status can never be overwritten** by a late worker. *Cost if wrong:* none.
- **Deleting a running question:** stop it if it's on this server, delete it, then release its remaining budget. A worker on another server copy stops on its own once it sees the question is gone. *Cost if wrong:* none.
- **Each AI call carries a fixed ID, and parallel calls claim budget before they start.** *Cost if wrong:* none.
- **Check-ins replace restart-time cleanup:** 20-second check-ins, a 90-second limit, and a cleanup every minute. *Cost if wrong:* if the database were unreachable for over 90 seconds, a healthy question could be marked interrupted.
- **Small follow-ups for later stages:**
  - the run page should keep retrying after a brief network error rather than stop
  - History should show up to 100 questions, not 20

  Both are being fixed when Stage D rebuilds the run page.

### Next stage
**Stage B: Literature tools.** These are the connections to PubMed, Europe PMC and Unpaywall, with a fixed list of allowed addresses, a shared cache of paper details, retraction checks, and test data recorded from the real services.

## Stage B: Literature tools ✅

**Date:** 28 Oct 2026 · **Commits:** `69bd87b..40ba020`, plus the test-freeze fix `ab4df0e` and the CI fixes `aef426f` and `07280aa` · **GitHub CI:** run 40614291845 on `07280aa`. Everything passed: tests with Postgres, lint, the security audit, the container build (including the browser/WASM build) and the smoke test.

### What was built (plain English)
- **A locked-down web client.** Incisive's code can contact only PubMed (NCBI), Europe PMC, Unpaywall and Anthropic. A free full-text link is allowed only if it came from a Europe PMC or Unpaywall response, and only once. Reviewers tried more than a dozen tricks to reach other sites, including look-alike letters, IP addresses, user@host addresses, redirects and http downgrades. All were refused and are now permanent tests. Downloads are capped at 5 MB, and brief network hiccups are retried.
- **PubMed connection.** It does search, paper details and abstracts. It stays under PubMed's limit of 3 requests per second (10 with a key), retries included. Retracted papers are detected two ways. The API key, when you add one, never appears in logs.
- **Europe PMC and Unpaywall.** Europe PMC search finds open-access papers and reads their full text in sections: abstract, methods, results, discussion. Unpaywall finds free versions by DOI and records whether a link is a PDF or a web page. Results from both databases are merged, and duplicates are removed by PMID, DOI or PMCID.
- **A shared cache of paper details.** It holds public bibliographic data only, with no record of who looked anything up. Entries are fresh for 30 days and deleted after 90. Refreshing a paper never wipes a saved abstract, and a retraction, once seen, stays marked.
- **Real test data.** Since you allowed the three sites, the tests use real recorded responses. These include ICG and anastomotic leak (including a free full-text paper), inguinal hernia mesh RCTs, a retracted paper, and three Unpaywall cases: free PDF, free page only, and paywalled.

### Tasks
| Task | Review verdict | Fix rounds | Commits |
|---|---|---|---|
| 5. Locked-down web client | Approved after 1 fix round | 1 | 69bd87b, cd24f35 |
| 6. PubMed and paper cache | Approved after 1 fix round | 1 | 8041144, c0d8a44 |
| 7. Europe PMC and Unpaywall | Approved after 2 fix rounds | 2 | c3a8d5c, c9bd7ca, 40ba020 |

### Two problems found along the way
- **A test freeze.** One test suite occasionally froze. The root cause was in the web framework: building the app more than once in the same running server makes any page that loads data wait forever. The live site builds it once, so it was never affected. The tests rebuilt it constantly, which is how it surfaced. It's now built once per process, a test guards this, and a one-minute watchdog means a freeze can never stall GitHub again. 33 stress runs on 2 CPUs passed.
- **GitHub failing for hours.** Once I made the failure readable, the reason was GitHub's build machine running out of disk. Compiling every test with full debugging information was too big. Builds now keep only the detail needed for readable errors, and the GitHub job deletes unused preinstalled tools first. GitHub is green again.

### Review findings (summary)
- **Web client:** network hiccups weren't retried, and the 5 MB cap relied on callers remembering it. Both are fixed.
- **PubMed:** retries could break PubMed's rate limit, and a summary-only refresh could wipe a saved abstract. Both are fixed.
- **Europe PMC and Unpaywall:** links didn't say whether they were PDFs, which Phase 2 can't read. Each link is now marked, web pages are preferred, and the marking is stored in the database. A temporary debugging file was accidentally included in a commit and has been removed.

### Decisions made during the stage (rulings)
- **Only the API responses can unlock a full-text link.** Never paper text or AI output. *Cost if wrong:* none.
- **Retractions are "sticky" in the cache.** A refresh can't un-retract a paper. *Cost if wrong:* a genuine un-retraction would need a manual fix.
- **Phase 2 reads full text only from Europe PMC's structured version.** Free PDFs and web pages are used as "abstract only". *Cost if wrong:* fewer full texts. Europe PMC's open-access set covers most free biomedical papers.
- **New database columns** for the cache: PMCID and the PDF flag. These are new migrations, and the earlier ones are never edited.

### Next stage
**Stage C: Finding the evidence.** Already under way:
- **Task 8, the budgeted Claude call:** done. Every AI call reserves its cost, stays charged even if the question is stopped mid-call, and paper text is wrapped as data.
- **Task 9, framing and search:** done. The question becomes a PICO and a PubMed search, refined up to 3 times. In a live check, Europe PMC went from 3 to 101 hits once its queries were translated.
- **Task 10, screening and retrieval:** being built.
- **Task 11, extraction with verbatim quotes:** next.
