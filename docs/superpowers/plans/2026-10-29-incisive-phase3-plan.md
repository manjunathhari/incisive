# Incisive · Phase 3 · Build Plan

**Spec:** `docs/superpowers/specs/2026-10-29-incisive-phase3-evidence-team-design.md`
**Process:**
- One builder and one reviewer per task. Money and safety tasks get full review rounds.
- Before every merge: a real-model smoke script, then one live question.
- Each stage ends in a PR, which merges only after the owner's go-ahead.

| Stage | Tasks | Ends with |
|---|---|---|
| 0. Faster releases | T0 | Deploy reuses cached layers (cargo-chef + buildx gha cache) |
| 1. Agent engine | T1 tool-use loop · T2 literature tools · T3 limits | Tested engine; smoke script for tool calls |
| 2. Quick mode | T4 researcher · T5 writer + answer schema · T6 basic answer page | Quick answers live |
| 3. Full mode | T7 extractor + appraiser + quote verification · T8 skeptic · T9 parallel orchestration | Full answers live |
| 4. Integrity + page | T10 number gate + revision · T11 full answer page + progress per role + Phase 2 answers readable | Finished product |
| 5. Proof | T12 gold-set runner v3 · T13 20-question comparison vs Claude chat · T14 tuning, retire Phase 2 engine | Baseline recorded |

## Tasks

**T0 Faster releases.** Dockerfile uses cargo-chef. The CI container job builds with buildx and a gha cache, and the deploy reads that cache. The safety checks in deploy.yml are unchanged.

**T1 Tool-use loop.**
- `LlmCall` gains a conversation-based `agent()` call. Its inputs are the system prompt, tools (name, description, input_schema), the max number of turns and a final output schema.
- Every turn is one budgeted logical call, with the same hold → spawn call+charge → settle path. Limits are respected between turns: cancel, wrap-up, hard stop.
- `tool_use` blocks are run by a tool registry, and the results go back as `tool_result` blocks. Paper text inside results is wrapped by `wrap_source`. Tool errors are returned to the model as error results, not as run failures.
- The final answer goes through structured outputs.
- claude-client gains the tool_result content block and the multi-turn message types.
- Tests use a scripted multi-turn LLM. A smoke script exercises a real two-turn tool call on Sonnet and Opus.

**T2 Literature tools.** These wrap the existing clients:
- `search_pubmed`
- `search_europepmc`
- `get_abstracts`
- `get_full_text`
- `find_related`: new; PubMed elink on eutils.

Each tool has input validation, result size caps and no hosts outside the allowlist. Results are compact JSON (id, title, year, journal, pub types, abstract when asked).

**T3 Limits.** Each mode's config gets money, time and turn caps per role. These go into RUN_LIMITS with defaults, and the deploy pre-check is updated. Graceful finish: the writer is always reserved for.

**T4 Researcher.** Its prompt is the prototype's, refined. It uses the T1 loop with the T2 tools, and its final structured output is the study list. In Quick mode it runs with fewer turns and abstracts only. It writes progress lines (searches, selected count).

**T5 Writer + answer schema.**
- AnswerV3 is the structured format from spec §2. Numbers sit in structured fields, each with its source study.
- The writer makes one structured call.
- In Quick mode it writes from the study list + abstracts only; in Full mode from the evidence table + skeptic output.
- Stored in run_answers with a version tag.

**T6 Basic answer page.** Renders AnswerV3: bottom line, outcome table, trials table, conflicts, applicability, what would change it, references, trail, cost and time. It is Leptos-escaped. Phase 2 answers still render.

**T7 Extractor + appraiser.**
- One structured call per study, 3–4 concurrent, using the Fix B bounded runner and its money rules.
- Code verifies every quote with the existing normaliser against the fetched abstract or full text.
- A failed quote gets one correction request. If it fails again, the number is dropped and the row notes it.

**T8 Skeptic.** Agent loop with ≤8 turns. Its structured output: contradictions with quotes, missed RCTs, guidelines, verdict, must-mention items.

**T9 Orchestration.**
- Full mode runs: researcher → (extractor ∥ skeptic) → writer → gate.
- Quick mode runs: researcher → writer → gate.
- Progress lines are written for each role. Cancel and hard-stop semantics stay as in Phase 2.

**T10 Integrity gate.**
- Every number in the text fields must be found in a verified quote or a structured field. If not, there is one revision call that lists the offenders. If it still fails, those sentences are removed and noted.
- References are checked live (the check.rs verify_refs logic) and retractions are flagged.

**T11 Full answer page.** Polished tables, per-role progress, mobile layout, Copy as text. No charts.

**T12 Gold-set runner v3.**
- Scores AnswerV3: landmark recall, references re-checked, number-gate stats, cost and time.

**T13 Comparison.**
- The owner's 20 questions are run through Incisive. The owner runs the same questions in Claude chat and pastes the answers.
- Both are scored against the answer keys.

**T14 Tuning + retire.** Prompts, limits and models are tuned on the results. The Phase 2 engine modules are removed; history rendering stays.
