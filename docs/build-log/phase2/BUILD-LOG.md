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
| C. Finding the evidence | 8–11 | ✅ complete (28 Oct) |
| D. Writing and checking | 12–13 | ✅ complete (28 Oct) |
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
**Stage C: Finding the evidence.** It's reported below.

## Stage C: Finding the evidence ✅

**Date:** 28 Oct 2026 · **Commits:** `6148d4a..c47feb8` · **GitHub CI:** run 40623407579 on `7a371d7`, all checks passed. That was the first run after the repo went public; earlier runs had stopped on GitHub billing.

### What was built (plain English)
- **A budgeted Claude call.** Every AI call reserves its likely cost before it starts and is charged the real cost afterwards. The charge still happens if you stop the question mid-call. If the charge can't be recorded twice in a row, the run stops; it never carries on unbilled. Paper text is always wrapped and marked as data, so instructions hidden inside a paper are never treated as commands.
- **Framing and search.** Your question is turned into a PICO (patients, intervention, comparison, outcome) and a PubMed search, which is refined up to 3 times if it finds too few or too many papers. The search is translated into Europe PMC's own syntax. In a live check this took Europe PMC from 3 hits to 101.
- **Screening.** Haiku screens papers 10 at a time and leans towards keeping a paper when unsure. Each exclusion gets a coded reason. The evidence level (meta-analysis, RCT, cohort and so on) comes first from the paper's official publication type, not its title, so a comment *on* a meta-analysis isn't ranked as one.
- **Retrieval.** Full text comes only from Europe PMC's structured open-access version. Everything else is read as abstract only.
- **Extraction with verbatim quotes.** Each finding must quote the paper word for word, within one section, starting at the beginning of a clause, and at least 20 characters long. Any number in the finding must appear in the quote. The quote is checked against the paper by code, not by the AI. Each paper's text is capped at 25,000 characters, in the order abstract, results, methods; the discussion is dropped first.

### Tasks
| Task | Review verdict | Fix rounds | Commits |
|---|---|---|---|
| 8. Budgeted Claude call | Approved after 1 fix round | 1 | 6148d4a, 5540d87 |
| 9. Framing and search | Approved after 1 fix round | 1 | c0783b4, 5a233fe |
| 10. Screening and retrieval | Approved after 1 fix round | 1 | 00e25ef, e89a624 |
| 11. Extraction | Approved after 2 fix rounds | 2 | 46b733a, 7d6b17b, c47feb8 |

### Review findings (summary)
- **Money:**
  - A failed charge could let a run carry on unbilled.
  - Stopping a question mid-call could lose the charge.
  - Both now fail safe.
- **Search:** PubMed-style search terms were sent to Europe PMC unchanged, so it found almost nothing. Now translated.
- **Screening:**
  - One malformed reply could throw away a whole batch of 10.
  - Titles could override publication types.
  - Both are fixed.
- **Extraction:** this was the most serious finding of the stage. A cut-off quote could pass the check. The reviewer showed how:
  - by dropping a minus sign or a leading digit;
  - by quoting "reduced mortality" out of "**not** reduced mortality";
  - by writing decimals with a middle dot (0·52).

  All of these are now blocked and kept as permanent tests.

### Decisions made during the stage (rulings)
- **A run stops if its cost can't be recorded.** *Cost if wrong:* an occasional run stops early because of a database hiccup.
- **Paper text sent for extraction is capped at 25,000 characters per paper** (the spec allowed up to 60,000). *Cost if wrong:* fewer quotable sentences from discussion sections.
- **Observational studies count as prospective cohorts only when the title says "prospective".** *Cost if wrong:* some real prospective cohorts are ranked lower ("other").

## Stage D: Writing and checking ✅

**Date:** 28 Oct 2026 · **Commits:** `dc414ee..d3d1127`, plus the CI fixes `7a371d7` and `149d77f` · **GitHub CI:** run 40632901642 on `149d77f`, all checks passed (tests with Postgres, lint, security audit, container build, smoke test).

### What was built (plain English)
- **Writing the answer.** Sonnet writes the answer from only two things: the PICO, and the verified findings with each paper's design, size and evidence level. It never sees the papers' full text at this point. The confidence wording ("strong", "moderate" and so on) is capped by code. "Strong" needs at least 2 RCTs, systematic reviews, meta-analyses or guidelines.
- **Checking every claim.**
  - The references are verified live against PubMed or Europe PMC first. Retracted or mismatched papers are removed.
  - Each claim then gets its own check against the verified quotes only.
  - The check confirms that each number belongs to the right measure and the right arm. For example, a p-value can't be presented as a risk ratio.
  - A claim that loses all its references is dropped.
- **The full pipeline.** Frame, then Search, Screen, Retrieve, Extract, Write and Check, with a live step line for each. Runs heartbeat, can be stopped, and finish gracefully when the time or money limit nears.
- **The answer page.** The order on the page is:
  1. "Decision support — the clinician decides."
  2. The bottom line, with ◐ marking claims that are only partly supported.
  3. Numbered references linking to PubMed or the DOI.
  4. The search trail.
  5. Edit & re-run.

  All AI text is shown as plain text; it can never run as code on the page.

### Tasks
| Task | Review verdict | Fix rounds | Commits |
|---|---|---|---|
| 12. Write and Check | Approved after 1 fix round | 1 | dc414ee, 5ba57c2 |
| 13. Pipeline and answer page | Approved after 2 fix rounds, plus a small final round | 3 | 13df628, ea2c8ba, 265cfc9, d3d1127 |

### Review findings (summary)
- **Check (critical):** a claim could stay marked "supported" after its supporting paper was removed. That would credit a finding to the wrong paper. Now:
  - references are verified before the claims are checked;
  - each claim is checked on its own, so one claim's evidence can't leak into another.
- **Pipeline:**
  - Running out of money mid-reading could produce a false "No relevant evidence found". It now says the run stopped early.
  - A run could spend its money gathering papers and then fail at the writing step. Every run now keeps back enough to write and check the answer before it gathers more.
- **Privacy:** Edit & re-run now carries the question after `#` in the address. That part of an address is never sent to the server, so your clinical questions stay out of the Google server logs.

### Decisions made during the stage (rulings)
- **The Quick limit is raised from ₹5 to ₹15 (your decision, 28 Oct).** At real Sonnet prices ₹5 couldn't even cover writing and checking the answer. At ₹15 a Quick run reads about 4 abstracts and checks its claims. Focused stays at ₹40; it reads about 6 full-text papers or 10 abstracts.
- **A run that hits its limit before reading anything is "partial".** "No relevant evidence found" is used only when every step ran.
- **Fewer than 2 distinct verified references means the answer is marked "insufficient evidence"**, however confident the wording.

### Problems found along the way
- **A server restart interrupted Task 13.** Its partial work was saved to a backup branch and finished by a fresh builder.
- **An intermittent CI failure.** A test that briefly installs a database trigger couldn't get the table lock in time on GitHub's slower machine. It now retries.

### Next stage
**Stage E: Quick, production and proof.**
- **Task 14:** the Quick/Focused toggle; Phase 2 production settings; an optional NCBI key.
- **Task 15:** the gold-set runner, plus a 5-question live check.
- **Then:** a whole-phase review, a pull request, and the merge on your go-ahead.

**Needed from you:** the 20 gold-standard questions (for Task 15) and an NCBI API key (optional, but it speeds up searches).
