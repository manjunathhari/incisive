# Incisive · Phase 3 · Evidence Team — Design Spec

**Status:** approved in conversation, 29 Oct 2026 (owner: Dr Manjunath Haridas)
**Supersedes:** the Phase 2 answer engine (frame → screen → extract → write → check). Phase 2's platform stays.

## 1. Why

The Phase 2 live check (28 Oct, Q13 robotic vs lap inguinal hernia) showed the fixed pipeline is weaker than its own model:

- **Search:** narrow, one-shot search missed key trials such as ROLAIS.
- **Writer:** it saw only quote snippets, capped at 6 one-sentence claims.
- **Verification:** it deleted anything that failed to match. It reported "35 findings dropped" and ended with 0 claims.

Claude chat gave a far better answer, though it had a broken link and an unmatched meta-analysis figure.

A prototype "evidence team", run on 29 Oct with Claude agents and PubMed tools, produced a clearly better answer on Q13 than Claude chat:

| | Prototype result |
|---|---|
| Trials found | 7 RCTs and 2 RCT-only meta-analyses (chat found 3 RCTs) |
| Numbers | 106/106 traced to verbatim source quotes |
| References | 17/17 resolved |
| Appraisal | Caught that DIRECT and ROLAIS come from the same centre |
| Self-correction | Fixed its own overstatements |
| Run cost | ~7.6 min, ~473k tokens (incl. prototype overhead) |

The owner approved this output format: "This output is good." Interactive charts and refinement come later.

**Goal:** build that evidence team into Incisive, with the owner's API key, budgets, privacy and verification.

## 2. What the surgeon gets (answer format, stored as structured data)

1. **Bottom line:** 2–3 sentences and an overall certainty (High / Moderate / Low / Very low, GRADE-style, with the reason).
2. **Outcome by outcome:** one row per outcome: outcome · RCT evidence · effect (numbers) · certainty · why.
3. **The trials:** one row per study: trial (year, journal) · design / n / setting · comparison · key results · appraisal.
4. **Conflicting evidence and why it differs.**
5. **Applicability:** lean or primary vs complex cases; India and cost context, stating what is unknown; implications for a teaching or fellowship programme where relevant.
6. **What would change this answer.**
7. **References:** title, journal, year, PMID and DOI. Every one is live-verified, with retractions flagged.

Plus a search trail, the live progress steps for each role, cost and time.

Numbers sit in structured fields (per arm, effect, CI, p, source study) so that Phase 4 can chart them without re-parsing text.

## 3. Roles

| Role | Model (default) | How it works | Output |
|---|---|---|---|
| Researcher | Opus 5.5 | Agentic tool loop: iterative searches, related articles, reads abstracts and full text | Study list: PMIDs, design, why selected, considered/rejected, possible contradictions, search log |
| Extractor + appraiser | Sonnet 5.5 | One structured call per study, 3–4 in parallel; code verifies every quote | Evidence row: design, n, setting, PICO, follow-up, blinding, primary endpoint and power, outcomes with effect + verbatim quote + source, appraisal (bias, power, learning curve, applicability) |
| Skeptic | Sonnet 5.5 | Agentic tool loop running in parallel with extraction; hunts contradicting or newer evidence and guidelines | Contradictions with quotes, missed RCTs, verdict, must-mention caveats |
| Writer | Opus 5.5 | One structured call from the verified table + appraisal + skeptic only | The structured answer in §2 |
| Integrity gate | Code (+ at most 1 revision call) | Number check, reference check, retraction check | Pass, or one revision; unverifiable numbers removed |

**Modes**
- **Quick:** researcher (lighter: fewer turns, abstracts only) → writer → gate.
- **Full:** every role.

## 4. Key mechanisms

- **Tool calling:** a multi-turn tool-use loop inside the budgeted call wrapper.
  - Every turn holds and charges as today: reservation, spawned charge, R-8d.
  - Tools run on our server through the allowlisted literature clients only.
  - `tool_choice` stays `auto`. Forced tool choice is rejected by Opus/Sonnet 5.5.
  - Each role's final output goes through structured outputs (`output_config.format`).
- **Literature tools:**
  - `search_pubmed`
  - `search_europepmc`
  - `get_abstracts(pmids)`
  - `get_full_text(pmcid)`
  - `find_related(pmid)`: new; PubMed elink on the allowlisted eutils host.
- **Quote verification:** every number in an evidence row must come with a quote that matches the fetched text under the existing normaliser. If a quote fails, one correction request goes back to the extractor. If it fails again, the number is dropped and the row says so. Nothing is invented and nothing is silently deleted.
- **Number gate:** every number in the writer's text must appear in a verified quote or a structured field. If not, there is one revision call listing the offending numbers. If it still fails, those sentences are removed and noted.
- **Prompt-injection safety:**
  - Paper text is always wrapped as data (`wrap_source`, `SOURCE_RULE`).
  - Tool results are treated as data.
  - The researcher cannot reach any host outside the allowlist.
- **Effort:** default (high) for the researcher and writer, `low` for simple structured sub-calls.

## 5. Limits (initial, for testing; tuned after the gold set)

| Mode | Money cap | Time | Turn cap |
|---|---|---|---|
| Quick | ₹60 | 4 min | researcher ≤8 tool turns |
| Full | ₹200 | 8 min | researcher ≤15, skeptic ≤8 tool turns |

The global monthly cap and the daily caps still apply. Graceful finish is unchanged in spirit: the writer is always reserved for.

## 6. Platform kept from Phase 2

- Sign-in (IAP) and the identifier filter.
- My questions, history, delete and 12-month retention.
- Budgets, caps and spend ledger.
- Run worker, heartbeat, recovery and cancel.
- Literature clients, allowlist, paper cache, retraction logic and quote normaliser.
- Deploy pipeline.

Old Phase 2 answers stay readable.

## 7. Retired

The Phase 2 steps (frame, screen, extract, write, check) are retired once Phase 3 passes the gold set.

## 8. How we prove it

- Gold-set runner updated for the new answer.
- The owner's 20 questions are run through both Claude chat and Incisive and scored against the answer keys:
  - landmark papers found
  - numbers correct
  - citation integrity
  - appraisal quality
  - time and cost
- Before every merge:
  - a real-model smoke script (Cloud Shell)
  - one end-to-end live question

## 9. Out of scope (Phase 4+)

- Interactive charts: outcome plots, trial comparison, forest-style displays from the structured numbers.
- Refining an answer with follow-ups ("focus on obese patients", "add cost-effectiveness").
- Alerts when new trials appear.
- Exports for journal club or patients.
- Cost reduction: caching extracted rows across questions, Sonnet for more roles, prompt caching.

## 10. Risks

- **Cost and time above target:** limits are configurable; measure, then tune (Phase 4).
- **Full-text access is limited:** PMC open access only, so abstract-only rows are labelled.
- **Agent loops wandering:** turn caps, budget and time limits, and a search log in the trail.
- **API format surprises:** a real-model smoke test before every merge.
