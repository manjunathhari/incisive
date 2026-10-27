# Incisive: Design Spec (v1)

*Working name. Tagline: "Cut to the evidence." (Previously "Surgeon Evidence Sandbox"; trademark and domain availability to be checked before launch.)*

**Date:** 26 October 2026 (revised: decisions added)
**Owner:** Dr. Manjunath Haridas
**Status:** Draft for review
**Mockups:** "Surgeon Evidence Sandbox" design canvas (4 screens: Evidence answer, Evidence table, Interactive forest plot + GRADE, Journal club)

---

## 1. Purpose

A specialist, agentic web app for practising surgeons, built on the Claude API, that does one job well: **turn a surgical question or a paper into appraised, cited, interactive evidence.**

The model is Claude Code. Claude Code is Claude focused on one domain, working through multi-step tasks with tools, showing its working, and letting the user steer. This sandbox applies the same pattern to surgical evidence and teaching.

### Goals (v1)
1. **Evidence Q&A:** answer clinical questions from PubMed, Europe PMC and society guidelines, with every claim linked to a verifiable source.
2. **Journal club:** appraise a single paper against the correct reporting checklist and produce a teaching pack for fellows.
3. **Interactive output:** evidence tables, study cards, forest plots, GRADE summary of findings, and exports, not just prose.
4. **Medical-only scope:** off-topic requests are refused.
5. **Usable inside Claude:** the same tools exposed as an MCP server.

### Non-goals (v1)
- No identifiable patient data, case notes, or clinical images.
- No patient-facing use.
- No final treatment recommendations. It is decision support, and the clinician decides.
- No hospital or EMR integration.
- No open sign-up or billing.

### Future (designed for, not built)
A patient-facing version: plain-language education, pre- and post-operative guidance, and escalation of red-flag symptoms. This will need consent, DPDP compliance, clinical sign-off on content, and a regulatory review (CDSCO Software as a Medical Device).

---

## 2. Users and access

| Role | Who | Capabilities |
|---|---|---|
| Owner | Dr. Haridas | Invites, verifies registration, sets caps, views logs |
| Surgeon | Invited practising surgeons (about 5–30) | Full use within personal caps |
| Fellow | Invited trainees | Full use, lower caps; supervisor visibility optional |

- Invite only; the owner checks registration before activation.
- Login by **passkey** (WebAuthn), with email magic-link as backup. No passwords.
- Users can view and revoke their signed-in devices.
- API costs are paid by the owner, with per-user daily and monthly caps.

### Budget
- Funded first by the **monthly API credits included with the owner's Claude Max plan** ($100/month on Max 5x, $200/month on Max 20x), claimed by linking a Claude Console organisation in Settings › Billing. Credits renew monthly, expire unused, and cover the Claude API.
- The Max subscription itself does not power the app: all usage goes through a Console API key.
- When credits run out and no purchased credits or auto-reload exist, API requests pause until the next cycle. This acts as a natural global hard cap.
- Before launch: confirm in Anthropic's Supplemental Credit Terms that the credits may be used for an app used by an invited group.
- **Default caps (configurable by owner):** owner reserve 30% of the monthly budget; remaining 70% split evenly across active surgeons; fellows receive half a surgeon's share. Per-query cost limit shown before long runs.

---

## 3. Architecture

**Principle:** Rust-first for safety, speed and long-term maintainability, with clean boundaries so any component can be replaced without a rewrite.

| Component | Language | Responsibility |
|---|---|---|
| Web server | Rust (axum) | HTTPS, sessions, passkeys, roles, rate limits, streaming agent steps to the browser |
| Gate | Rust | Medical-only classification (small Claude model call) and patient-identifier filter |
| Agent loop | Rust | Claude API client (HTTPS/JSON), planning, tool use, checkpoints, step/time/cost limits |
| Tools | Rust | PubMed (E-utilities), Europe PMC, Unpaywall, guideline library search, citation verifier |
| Guideline library | Rust | Curated society guidelines and free resources, indexed for keyword and semantic search |
| Audit log | Rust | Append-only, hash-chained log of every query, tool call and answer |
| MCP server | Rust (official MCP Rust SDK) | Exposes the tools to the Claude app |
| Front end | Rust → WebAssembly (Leptos full-stack, served by the same axum server) | Agent panel and results canvas |
| Science worker | Python (isolated container, no internet) | Meta-analysis (pooled estimates, I², forest plot data) and difficult PDF parsing |

```
Browser ── Leptos (Rust → WebAssembly)
   │  HTTPS + streaming
Rust core ── auth · gate · caps · audit · agent loop · tools · MCP
   │  internal API (numbers/text in, results out)
Python science worker ── stats · PDF parsing   [replaceable]
   │
External, allowlisted: Claude API · PubMed · Europe PMC · Unpaywall
```

**Boundaries**
- The browser talks only to the Rust core.
- The Python worker talks only to the Rust core, through a narrow, versioned internal API. If Rust libraries mature, it can be replaced without other changes.
- The agent can reach only allowlisted domains.
- Standards: MCP now; FHIR planned for any future clinical-record work.

**Note:** there is no official Anthropic Rust SDK (as of design date), so v1 includes a thin, well-tested Rust client for the Messages API.

### Hosting (decided)

| Layer | Host | Region | Why |
|---|---|---|---|
| Rust app: Leptos full-stack (screens + axum server), agent loop, gate, tools, MCP | **Google Cloud Run** (service `incisive-app`) | **Mumbai, asia-south1** | Runs any container; scales to zero; requests up to 60 minutes, so long agent runs fit; always-free monthly allowance (180,000 vCPU-s, 360,000 GB-s, 2M requests) covers the pilot |
| Python science worker | **Google Cloud Run** (service `incisive-science`, internal ingress only, no internet egress) | Mumbai, asia-south1 | Isolated; reachable only from `incisive-app` |
| Database, file storage, vector search for the guideline library | **Supabase** (Postgres + Storage + pgvector) | **South Asia (Mumbai), ap-south-1** | India data residency; owner already has an account; Rust connects as standard Postgres |
| Container images | Google Artifact Registry | asia-south1 | 500 MB free; Rust images are small |

- **Deploys:** automatic from GitHub on merge to `main` (build image → push → deploy both services).
- **Cost controls:** Google Cloud budget alert (initially ₹500/month); request-based billing so the app costs nothing while idle.
- **Caveats:** a Google Cloud billing account is required even for free-tier use; free-tier limits can change with 30 days' notice; Supabase free projects may pause when inactive, so production use moves to a paid Supabase tier.
- **Why not Vercel Functions:** agent runs last minutes and stream continuously; Vercel caps function duration (300 s default, 800 s on Pro) and its 30-minute beta excludes Rust. Vercel remains available for the owner's TypeScript projects.
- **Fallback:** the app ships as a standard container, so it can move to another host (e.g. Fly.io Mumbai) by changing deployment settings only.

---

## 4. Agent workflow

The agent follows a visible loop: **plan → act with tools → check → present.** Every step streams to the agent panel. Checkpoints (✋) let the user edit before the agent continues; in auto mode it proceeds unless stopped.

### Mode 1: Evidence Q&A
1. **Understand:** gate confirms a medical question; agent drafts PICO. ✋
2. **Plan search:** MeSH-based queries, date limits, study-type filters. ✋
3. **Search:** PubMed, Europe PMC and the guideline library in parallel; deduplicate.
4. **Screen:** titles and abstracts against PICO; one-line reason for each inclusion or exclusion; PRISMA-style counts. ✋
5. **Retrieve full texts:** Europe PMC open access, Unpaywall, or the user's library proxy. Abstract-only studies are flagged.
6. **Extract:** design, n, population, intervention, outcomes, effect sizes.
7. **Appraise:** RoB 2 (RCTs), ROBINS-I (observational); GRADE certainty per outcome.
8. **Synthesise:** bottom line with citations on every claim; disagreements shown; pooling via the science worker when studies are comparable.
9. **Verify:** an independent checking pass (fresh context, no access to the drafting) confirms every PMID resolves and each claim is supported by its source. Unsupported claims are removed or flagged.
10. **Present** on the results canvas.

### Mode 2: Journal club
1. Input a PMID, DOI or PDF.
2. Detect the study design and apply the matching checklist: CONSORT, STROBE, PRISMA, or CASP.
3. Extract key results; assess strengths, weaknesses and bias.
4. Context mini-search: earlier and later evidence, replication.
5. Teaching pack: one-page summary, five discussion questions, "would it change practice?", exportable slides.

### Guardrails
- Per-query limits on steps, time and cost (owner-configurable).
- When evidence is thin or absent, the agent says so rather than filling gaps from memory.
- Every output is labelled as decision support.

---

## 5. Interactive canvas

Layout: **agent panel** (left: steps, checkpoints, PICO, steering box) and **results canvas** (right: tabs). On phones these become swipeable tabs.

| Tab | Content |
|---|---|
| Answer | Bottom line, GRADE certainty badge, claims with citation chips; tapping a chip shows the source card with the supporting sentence |
| Evidence table | Sortable and filterable by design, year, n, outcome, country; tapping a row opens the study card |
| Studies | Study cards with population, intervention, outcome, effect, risk-of-bias domains, links |
| Charts | Interactive forest plot (toggle studies; exclude high risk of bias; RCTs only; pooled estimate and I² recalculate live), PRISMA flow diagram, evidence timeline |
| GRADE | Summary of findings: outcome, studies, effect, certainty, reasons for downgrading |
| Journal club | Checklist items marked reported / partly / not reported, with location in the paper; appraisal; teaching pack |
| Export | PDF summary, teaching slides, RIS/BibTeX, CSV; save and re-run on a schedule; share read-only with the group |

**Design rules:** every number traces to its source in one tap; clean clinical styling with dark mode; readable on a phone between cases; WCAG AA contrast, colour never the only signal.

---

## 6. Privacy and security

### Patient data
- v1 policy: questions about evidence, not identifiable patients.
- A Rust **de-identification filter** inspects every message before it reaches Claude (names, UHIDs, phone and Aadhaar numbers, dates of birth, addresses). Matches are blocked and the user is shown what to remove.
- Uploads are limited to published papers.

### Storage
- India-region hosting; encryption at rest; per-user data separation.
- Default retention 12 months; users can delete their history at any time.
- Before launch: confirm Anthropic's current API data-retention terms and whether zero-data-retention is available for the account.

### Library access
- Users store only their institution's proxy link (EZproxy or OpenAthens), never credentials.
- Paywalled full texts open through the user's own browser session.
- Processed full texts are private to that user, never shared, and auto-deleted after a set period.

### Keys and spend
- Claude API key held only on the server in a secrets store.
- Per-user daily and monthly caps with hard stop and owner alert; rate limiting.

### Agent safety
- Read-only agent: search and read only; no actions outside the app.
- Domain allowlist for tools.
- All fetched content treated as data, never as instructions (prompt-injection defence).
- Python worker in an isolated container with no internet access.

### Audit and hygiene
- Append-only, hash-chained audit log; any answer can be replayed.
- Automated dependency vulnerability scanning on every build; backups; incident plan (contacts, account lockout).

---

## 7. Testing and validation

### Software tests
- Rust unit and integration tests for all components.
- Adversarial generated tests for the de-identification filter and the medical-only gate.
- Science worker must reproduce published meta-analyses (e.g. Cochrane reviews with raw data) to two decimal places and match R *metafor*.
- Citation checker tested against fabricated, mistyped and retracted PMIDs.

### Gold-standard question set
About 100 questions across HPB, colorectal, upper GI, hernia, bariatric and general surgery, each with an expected answer and the landmark studies it must find. Includes settled, contested, no-evidence, off-topic, identifier-containing, and prompt-injection cases. Authored by the owner and two or three colleagues.

### Release criteria

| Measure | Threshold |
|---|---|
| Citations that resolve to real records | 100% |
| Sampled claims supported by the cited source | ≥ 95% |
| Landmark studies found | ≥ 80% |
| Expert rating "accurate and safe" | ≥ 90% |
| Correctly reports insufficient evidence | ≥ 90% |
| Off-topic or identifier messages blocked | ≥ 99% |
| Expert-flagged potentially harmful answers | 0 |
| Time and cost per query | Within owner-set limits |

### Expert review
Two surgeons rate each answer blind to system version (accuracy, completeness, harm potential); a third resolves disagreements.

- **Reviewer anonymity:** reviewers are appointed privately by the owner. The system records only reviewer codes (R1, R2, R3), never names, so reviews are blinded in both directions and reviewer identity is not stored in the app.
- Reviewers also co-author the gold-standard question set; question authorship is likewise recorded by code.

### Journal club validation
Appraisals of papers with well-documented flaws compared with expert appraisals (Cohen's kappa).

### Regression gate
The full question set runs automatically after every prompt, model, or tool change. A drop below any threshold blocks release.

### Pilot and monitoring
1. Owner only (about 2 weeks) → 2. three to five colleagues → 3. full invite group.
- "Flag as wrong" on every answer; flagged cases are reviewed and added to the question set.
- About 5% of answers sampled monthly for expert review; alerts on citation failures, gate blocks, cost spikes, or model version changes.

The validation study is designed to be publishable.

---

## 8. Build phases

| Phase | Deliverable |
|---|---|
| 0 | Repository, CI, hosting, secrets, Rust Claude client |
| 1 | Rust core: auth (passkeys, invites, roles), gate, de-identification filter, caps, audit log |
| 2 | Agent loop with PubMed, Europe PMC, Unpaywall, citation verifier; Evidence Q&A end to end (text output) |
| 3 | Leptos front end: agent panel, Answer, Evidence table, Studies tabs |
| 4 | Python science worker; forest plot, GRADE, PRISMA |
| 5 | Journal club mode and exports |
| 6 | Guideline library and library-proxy access |
| 7 | MCP server |
| 8 | Validation against release criteria; staged pilot |

---

## 9. Risks

| Risk | Mitigation |
|---|---|
| Hallucinated or misattributed citations | Independent verification pass; 100% citation-resolution gate |
| Patient identifiers entered | Pre-API filter; user education; blocked by design |
| Leptos/Rust front-end development speed | Accepted trade-off; component boundaries allow change later |
| Thin Rust statistics ecosystem | Isolated Python worker, replaceable |
| Paywalled content licensing | Per-user access only; no sharing; auto-deletion |
| Model version changes alter behaviour | Regression gate on every change |
| Cost overrun | Per-user and global caps with alerts |

---

## 10. Decisions (26 Oct 2026)

1. **Guideline library: all relevant societies.** Initial scope covers GI, HPB, colorectal, hernia, bariatric, oncology and perioperative care, including: SAGES, EAES, ERAS Society, ESCP, ASCRS, IASG, ACS, ESSO, ESMO, WSES, European Hernia Society, IFSO, ASMBS, IHPBA/AHPBA, NICE, WHO, and Cochrane abstracts. **Licensing rule:** openly published guidelines are indexed in full; guidelines whose terms restrict copying or reuse (some require login or prohibit redistribution) are stored as metadata and links only, and opened by the user from the source.
2. **Reviewers: blinded and anonymous.** Owner appoints privately; system stores reviewer codes only (see section 7).
3. **Hosting:** Google Cloud Run in Mumbai (Rust full-stack app and isolated Python worker) + Supabase in Mumbai (data). See section 3.
4. **Budget:** Claude Max monthly API credits via a linked Console organisation, with default caps as in section 2.
5. **Name:** Incisive ("Cut to the evidence"), subject to trademark and domain checks.
6. **Agent loop: built in Rust on the Claude Messages API.** Alternatives considered and declined for v1: the Claude Agent SDK (Claude Code's harness as a library; Python/TypeScript only, coding-oriented built-in tools) and Claude Managed Agents (hosted harness; beta, data regions unspecified, not currently eligible for zero-data-retention). Managed Agents may be revisited for later phases.
7. **Login: Google sign-in via Identity-Aware Proxy (27 Oct 2026).** Replaces invites and passkeys for v1: access is an allow-list of Google accounts (IAP policy + OAuth test users); the app verifies IAP's signed JWT and maps the email to `incisive.users` (role, caps). Roles, caps and audit from section 2 and 6 are unchanged. A full login system is deferred to the patient-facing version.

## 11. Remaining open items (non-blocking)
- Confirmation of Anthropic's Supplemental Credit Terms for group use of Max API credits.
- Trademark and domain check for "Incisive".
