# Incisive · Phase 2: Evidence Agent · Design

**Status:** design approved in conversation 27 Oct 2026; awaiting written-spec review.
**Parent spec:** `surgeon-evidence-sandbox-design.md` (sections 4, 6, 7, 8).
**Builds on:** Phase 1 (live): identity → identifier filter → medical gate → spending reservation → audit log. Phase 1 currently ends with "Accepted. The evidence agent arrives in Phase 2." Phase 2 replaces that with a real answer.

---

## 1. Purpose and success

Turn an accepted surgical question into an **appraised, cited, verified text answer**, from PubMed and Europe PMC, with every claim traceable to a quoted source.

Phase 2 is done when:
- A Focused question produces the answer page in section 4 within its limits (₹40, 5 minutes).
- Every citation in every answer resolves to a real PubMed record (100%).
- The owner's starter gold set (20 questions) has been run, and the report has been reviewed with the owner. The landmark-found rate and the removed-claim rate are recorded as the Phase 2 baseline. The release thresholds in the parent spec (≥ 80% landmark, ≥ 95% claims supported) apply at Phase 8, not here.
- All CI checks are green, the whole-phase review is done, and it is merged on the owner's go-ahead.

### Decisions (owner, 27 Oct 2026)
| # | Decision |
|---|---|
| D1 | Two depths, **Quick** and **Focused**, as one pipeline with different settings. Focused is built first; the Quick toggle comes at the end of Phase 2. |
| D2 | **No pauses.** The agent runs straight through. The answer shows the PICO and query it used, with **Edit & re-run**. |
| D3 | **History is kept**, private per user, deletable, with automatic deletion after 12 months. |
| D4 | **Tiered models:** Haiku screens; Sonnet frames, searches, extracts, writes and checks. Each step's model is a setting. |
| D5 | **Live step list** while running; the run continues if the page is closed. |
| D6 | **Per-question limits:** Quick ₹15 (owner ruling 28 Oct; was ₹5), Focused ₹40, 5 minutes for both. Near a limit the run finishes gracefully. Limits are owner settings. |
| D7 | **Hybrid agent:** fixed steps, with agent-style freedom only inside Search (up to 3 query refinements). |

### Out of scope for Phase 2
The interactive canvas and agent panel (Phase 3); forest plots, formal GRADE and PRISMA diagrams (Phase 4); journal club and exports (Phase 5); the guideline library and library-proxy full texts (Phase 6); the MCP server (Phase 7); full validation with blinded reviewers (Phase 8).

---

## 2. The pipeline

A question that passes Phase 1 becomes a **run**. A run executes these steps in order, and each step appends a line to the run's step list.

| # | Step | Model | What it does |
|---|---|---|---|
| 1 | **Frame** | Sonnet | Writes the PICO, the outcomes that matter, and any limits the user stated (design, date range). Output is strict JSON. |
| 2 | **Search** | Sonnet + tools | Drafts a PubMed query (MeSH and filters) and runs it. It sees the hit count and the top 20 titles, and may refine the query up to N times (Quick 1, Focused 3). The final query also runs on Europe PMC. Results are merged and de-duplicated by PMID, then DOI. |
| 3 | **Screen** | Haiku | Judges each title and abstract against the PICO: include or exclude, plus a one-line reason. Batched. Included papers are ranked by evidence level: guideline / systematic review / meta-analysis > RCT > prospective cohort > other. |
| 4 | **Retrieve** | none | **Focused only.** Fetches the full text of the top K included papers from Europe PMC open access, then Unpaywall's open-access location. If neither has it, the abstract is used, marked "abstract only". |
| 5 | **Extract** | Sonnet | Per paper: design, n, population, intervention, comparison, key outcomes with effect sizes, and the **verbatim source sentence** for each extracted number or finding. |
| 6 | **Write** | Sonnet | Writes the answer **only** from the extracted facts. Every claim carries citation numbers and a confidence word (*strong*, *moderate* or *limited*). |
| 7 | **Check** | Sonnet, fresh context | Gets each claim with its cited quotes, never the drafting conversation, and returns *supported*, *partly supported* or *unsupported* per claim. Code separately verifies every PMID live (section 5). Unsupported claims are removed; partly supported claims are marked ◐. |
| 8 | **Present** | none | Saves the structured answer and marks the run done. |

### Depth settings (owner-editable)
| Setting | Quick | Focused |
|---|---|---|
| Search refinements | 1 | 3 |
| Records screened (top-ranked by database relevance) | 20 | 100 |
| Papers used for extraction | 5 (abstracts) | 10 (full text where free) |
| Retrieve step | skipped | runs |
| Check step | **always** | **always** |
| Cost limit | ₹15 | ₹40 |
| Time limit | 5 min | 5 min |

**Thin evidence:** if fewer than 2 relevant studies are included, the answer says **"Insufficient evidence"**, lists what was found, and does not draw a conclusion.

**Graceful finish:** before each step, and between batches inside Screen, Retrieve and Extract, the runner compares spend and elapsed time against the limits. It skips straight to Write and Check once spend reaches 70% of the cost limit or 3 minutes have elapsed, so that Write and Check always have budget left. The answer is then marked "search stopped early (cost limit / time limit)".

---

## 3. Running in the background

- **Send** (after the Phase 1 checks) creates a `runs` row with status `queued` and returns the run id at once. The UI then polls `GET /api/runs/{id}` every 2 seconds for status and new step lines.
- A background worker, a tokio task in the same Cloud Run service, executes the run. Closing the page doesn't stop it. The answer appears under **My questions**.
- **Cloud Run:** switch to CPU always allocated (`--no-cpu-throttling`), keeping min instances at 0. **Before switching, the cost is confirmed with the owner.**
- **One active run per user.** A second Send while a run is queued or running returns `{"status":"run_in_progress"}`: "You already have a question running."
- **Money:**
  - The Phase 1 spending reservation takes the run's full cost limit up front (₹15 or ₹40).
  - Each Claude call records its actual cost as a `charge` against the run.
  - When the run ends, the unused remainder of the reservation is released.
  - The per-user and global caps are therefore never overrun by more than Phase 1's documented single-call bound.
- **Restart recovery:** on startup, any run left `queued` or `running` is marked `interrupted` ("Interrupted — please re-run"). Its charges so far stand and its reservation remainder is released.
- **Time limit:** 5 minutes, enforced server-side (section 2, graceful finish). There is also a hard stop at 6 minutes, which marks the run `failed` with whatever was charged.
- **Audit (Phase 1 log):** the log records run started, each Claude call charged, run finished (with status, totals and counts), and run deleted. **Never question or answer text.**
- **Statuses:** `queued`, `running`, `done`, `partial` (answer produced but a step degraded), `failed`, `interrupted`.

---

## 4. The answer

The answer is stored as structured JSON (`run_answers.body`) and rendered as plain, phone-friendly text in Phase 2. Phase 3 redraws the same data as the canvas.

1. **Header:** the question; the Quick/Focused badge; the date (IST); **"Decision support — the clinician decides."**
2. **How I read your question:** the PICO and the final PubMed query, with **Edit & re-run**. Edit & re-run opens the question box prefilled with the original question. Sending it creates a new run that goes through all the Phase 1 checks again.
3. **Bottom line:** 3–6 sentences, with citations [n] and a confidence word for each claim. Disagreement between studies is shown explicitly. Insufficient-evidence wording is used when it applies.
4. **Key studies** (Focused): # · study (first author, journal, year) · design · n · main finding with effect size · source (full text / abstract only).
5. **References:** numbered, with title, journal, year, PMID and DOI links, and study type. Retracted papers never appear here.
6. **Check result:** e.g. "14 claims checked: 13 supported, 1 partly supported ◐, 0 removed. 9/9 references verified."
7. **Search trail:**
   - hits per database; screened, included and excluded counts, with the top exclusion reasons
   - any query refinements and their stated reasons
   - an early-stop notice, if there was one
   - any database that was unavailable
8. **Cost and time:** e.g. "₹18.40 · 2 min 41 s".

---

## 5. Safety

- **Address allowlist, enforced in code.** The HTTP client refuses every host except these:
  - `eutils.ncbi.nlm.nih.gov`
  - `www.ebi.ac.uk` (Europe PMC REST)
  - `api.unpaywall.org`
  - `api.anthropic.com`
  - open-access full-text hosts, but only when the URL came from a Europe PMC or Unpaywall response (`europepmc.org`, `www.ncbi.nlm.nih.gov/pmc`, or the `url_for_pdf`/`url` that Unpaywall returns, restricted to https)
- **Fetched text is data.** Abstracts and full texts are passed inside `<source id="…">…</source>` wrappers. Every prompt states that wrapped content is never an instruction. The model's only actions are search queries, and those are validated before use: allowed PubMed syntax characters, ≤ 1,000 characters, no URLs, no newlines.
- **No answering from memory.** Write receives only the extracted facts and their quotes. Check removes any claim without a supporting quote.
- **Citation verification by code.**
  - Every cited PMID is fetched live from PubMed (`esummary`).
  - If it doesn't resolve, or its title or DOI doesn't match the paper that was extracted, the reference and its claims are removed.
  - Retractions (PubMed publication type "Retracted Publication", or a "Retraction of" link) are excluded at Screen and re-checked at Check.
- **Full-text size limits:** at most 5 MB per download and 60,000 characters of extracted text per paper sent to Extract, in sections (abstract, methods, results), to bound cost.
- **Identifier policy:** the question has already passed the Phase 1 filter. Search queries are built from the PICO, so the user's free text is not forwarded verbatim to NCBI or Europe PMC.
- **Failure behaviour:**
  - database down: 2 retries, then continue with the other database; both down → run `failed`, with only the cost so far charged
  - no full text: use the abstract
  - bad model output: 1 retry, then a safe fallback (screen → exclude; check → unsupported, so the claim is removed)
  - nothing relevant: "No relevant evidence found", with the trail

---

## 6. Storage and privacy

New migration `0005_runs.sql`, all in schema `incisive`:

| Table | Key columns |
|---|---|
| `runs` | `id uuid pk`, `user_id`, `question text` (post-filter), `mode` (`quick`/`focused`), `status`, `cost_paise`, `limit_paise`, `started_at`, `finished_at`, `created_at`, `error_kind` |
| `run_steps` | `run_id`, `seq`, `at`, `kind`, `line text` (the human-readable step line; never contains the question verbatim) |
| `run_answers` | `run_id pk`, `body jsonb` (section 4 structure, including the supporting quotes) |
| `paper_cache` | `pmid pk`, `doi`, `title`, `authors`, `journal`, `year`, `pub_types`, `abstract`, `oa_url`, `retracted bool`, `fetched_at`. Public bibliographic data only, with no user linkage; entries refresh after 30 days. |

- Full texts are **not stored**. Only the supporting quotes inside `run_answers.body` are kept.
- **Access:**
  - A user sees only their own runs. Another user's run id returns 404, owner included.
  - The owner's Activity page shows counts and costs across users, never text.
- **Delete:** a single run, or "Delete all my history", removes `runs`, `run_steps` and `run_answers` rows. Audit entries remain, and they never held text.
- **Retention:** a daily cleanup deletes runs older than 12 months, then paper_cache entries older than 90 days that are unreferenced.
- **External services receive:**
  - Anthropic: the filtered question and source text
  - NCBI and Europe PMC: queries built from the PICO
  - Unpaywall: DOIs plus the contact email
- **New configuration:**
  - `NCBI_API_KEY` (optional secret; raises the PubMed limit from 3 to 10 requests/s)
  - `UNPAYWALL_EMAIL`
  - `AGENT_MODELS` (JSON mapping step to model; default Haiku for screen, Sonnet otherwise)
  - `RUN_LIMITS` (JSON of the depth settings in section 2)
- The prod startup check extends to "every model in `AGENT_MODELS` is priced in `MODEL_PRICES`".

---

## 7. Testing and validation

**Automated, in CI, with no network:**
- **Recorded fixtures:** real PubMed, Europe PMC and Unpaywall responses for about 5 topics, saved as test files.
- **Fake Claude:** scripted responses per step. These include malformed JSON, invented PMIDs, claims with no quote, and an abstract containing a prompt-injection string.
- **Tests:**
  - query validation; de-duplication; evidence-level ranking; retraction exclusion
  - citation verification: fabricated, mistyped, mismatched and retracted PMIDs are all removed
  - graceful finish at the cost limit and at the time limit; the 6-minute hard stop
  - one run per user; restart recovery; per-call charging and release of the remainder
  - history privacy (cross-user 404); delete and delete-all; 12-month cleanup
  - the run completes with no client connected
  - the allowlist refuses other hosts, including a redirect to another host

**Manual live check before merge:** 5 real Focused questions with real services. Answers are inspected by eye; cost and time are recorded in the build log.

**Owner gold set (starter):** 20 questions across HPB, colorectal, upper GI, hernia and bariatric. Each lists the landmark studies or guidelines a good answer must find.
- A script runs all 20 on Focused and reports:
  - landmark-found %
  - citation-resolve % (must be 100%)
  - claims removed by Check
  - cost and time per question
- The report is reviewed with the owner before Phase 2 is called done. It then becomes the regression baseline.
- **The questions don't block the build.**

---

## 8. Build stages

These follow the Phase 1 process: subagent implementer → independent reviewer → fix rounds. Each stage gets a report in `docs/build-log/phase2/BUILD-LOG.md` (mirrored to the Project), and a summary message goes to the owner after each stage.

| Stage | Contents | Visible result |
|---|---|---|
| **A. Runs and history** | migration 0005 (runs, steps, answers); run lifecycle and background worker; polling API; one-run rule; per-call charge and release; restart recovery; My questions (list, view, delete, delete all); retention job; placeholder steps | End-to-end flow with a dummy answer |
| **B. Literature tools** | allowlisted HTTP client; PubMed (esearch, esummary, efetch); Europe PMC (search, OA full text); Unpaywall; paper_cache; retraction detection; recorded fixtures | Tested tools |
| **C. Finding the evidence** | Frame; Search with refinement loop; Screen (Haiku, batched); Retrieve; Extract with quotes; graceful-finish checks | Real searches and counts in the step list |
| **D. Writing and checking** | Write; Check (fresh context); live PMID verification; answer rendering | Real Focused answers |
| **E. Quick, production, proof** | Quick toggle; `AGENT_MODELS`/`RUN_LIMITS` configuration and deploy wiring; the CPU-always-allocated switch (after the cost is confirmed); NCBI key; 5-question live check; gold-set script and report; whole-phase review; merge on go-ahead | Live |

**Needed from the owner:** an NCBI API key (Stage E), the Unpaywall contact email (Stage B), and the 20 starter gold questions (before the end of Stage E).

---

## 9. Risks

| Risk | Mitigation |
|---|---|
| The search misses landmark papers | Refinement loop; the gold-set report measures this; tuning is a later prompt change, not a redesign. |
| Hallucinated or misattributed claims | Write only from quotes; fresh-context Check; code-level PMID verification; drop when in doubt. |
| Cost creep per question | Reservation of the full limit; per-call charges; graceful finish at 70%; Haiku for screening. |
| Prompt injection in paper text | Source wrappers; query validation; address allowlist; no actions beyond search. |
| Cloud Run background work throttled | CPU always allocated; restart recovery; hard stop. |
| PubMed rate limits | NCBI key; paper_cache; request pacing (≤ 3/s without a key, ≤ 10/s with one). |
