# Incisive evidence chat: design

**Date:** 2 Nov 2026 · **Status:** designed with the owner in chat, awaiting owner review of this written spec
**Branch:** `evidence-chat` · **Team state:** `.10x/decisions/product-manager/evidence-chat.md` (C1–C10)
**Builds on:** memory layer v0.7.0 (records, topics, Gemma embeddings live since 2 Nov)

## 1. Purpose

A **Chat** option next to *Answer | Expert opinion* on a run page. The user asks questions in plain conversation and gets answers **only from that run's evidence** — its answer, Expert Opinion, the papers it selected and the quotes it verified — plus answers the owner has reviewed for the topic. Gemma 3 4B on the owner's VPS writes the replies; code chooses what Gemma may read and checks every reply. Conversations are stored; nothing trains a model.

## 2. Owner decisions

| # | Decision |
|---|---|
| C1 | For surgeons or patients: one conversational chat with a **Clinical / Plain language** switch. |
| C2 | Build order: chat → Topics (PR B) → gate (PR C). The three parked Gemma hardening items fold into this build. |
| C3 | Text first; voice next (browser speech, later). |
| C4 | Each conversation is private to its user; the owner can read all. Sign-up notice says so. A reply joins the knowledge base only after owner approval. Chats follow the 12-month rule. |
| C5 | Scope: this run only (+ owner-approved Q&As for its topic). |
| C6 | Chat only on Full runs with a finished Expert Opinion (runs with a memory record). |
| C7 | Personal questions: answer from the evidence for people in that situation, end with "Your own surgeon can tell you what this means for you", never "you should". Code adds a fixed safety line for warning symptoms. |
| C8 | Gemma is never called without passages. No relevant passage → code replies "Not covered in this evidence." |
| C9 | The knowledge base includes the selected papers' abstracts, verified quotes and reference details, kept permanently with the record. |
| C10 | During the run Gemma writes its **prior** (what it believes about the question). After the run each prior claim is reconciled against the evidence. Prior and reconciliation are **owner view only**; the chat never uses the prior. |

## 3. Evidence that shaped the design (VPS tests, 2 Nov, gemma3:4b, 2 CPU)

- **With passages:** warm reply ≈ 15 s (268 prompt tokens at 42.7 tok/s; 103 out at 11.8 tok/s); first load +8.6 s. Two of three sentences faithful with citations and uncertainty kept; one used a robotic-vs-laparoscopic passage to answer robotic-vs-open; jargon unexplained.
- **Without passages:** three invented references (none found in PubMed by journal/volume/page), invented stay figures (7–10 vs 10–14 days; 7.5 vs 10.2), all uncertainty dropped.
- Consequences: few, comparison-matched passages; strict prompt; code checks; never call without passages; keep the model warm.

## 4. Knowledge base per run

Built by code from stored data only (no engine change). Each **passage** is a short text with a source label:

| Kind | From |
|---|---|
| `bottom_line` | answer bottom line / certainty |
| `outcome` | one row of the outcome table (outcome, effect text, numbers, certainty, why) |
| `evidence` | one row of the evidence table |
| `conflict`, `applicability`, `what_would_change` | answer sections |
| `opinion` | Expert Opinion: verdict, "My view on why", each At a glance line, each era note, decisions |
| `abstract` | each selected paper's abstract (from `paper_cache`), split into ≤ 120-word pieces, with `[ref n]` |
| `quote` | each verified quote (extractor, skeptic), with `[ref n]` |
| `reference` | title, journal, year, design for each reference |
| `owner_qa` | owner-approved Q&As for the record's topic (§7) |

Each passage carries: `ref_n`/`pmid` when it comes from a paper, a **comparison tag** (normalised intervention–comparator pair, from the outcome group or the record's pairs; NULL when unknown), the text, a Gemma vector, and a full-text index.

**When it is built:** in the memory job, right after the record is written (same loop). The chat becomes available within about a minute of the Expert Opinion appearing; until then the option shows "Getting ready". A sweep fills missing vectors (same advisory-locked sweep as records).

## 5. Answering a question

1. **Pre-checks (code):** daily limit; one reply in flight per user; warning-symptom keywords (fever, bleeding, severe or increasing pain, chest pain, breathlessness, vomiting, wound redness or pus, unable to pass urine…) → set the safety line.
2. **Comparison detection (code):** match the question's words against the record's arm names → the asked comparison, or none.
3. **Retrieval (code):** hybrid score (meaning + words) over the run's passages and the topic's `owner_qa`. If a comparison was asked, passages tagged with a *different* comparison are excluded. Keep the top 3 (minimum score threshold).
4. **No passage above threshold → "Not covered in this evidence."** Gemma is not called (C8).
5. **Gemma (`/api/chat`, streamed):** system rules — answer only from the numbered passages; cite like [1]; stay within the comparison asked; if not covered say exactly "Not covered in this evidence"; never "you should"; no author/year/journal references. Plain mode adds: explain every abbreviation, short sentences, no statistics jargon. Context: the passages, the question, the last 3 turns. `num_predict` 300, `keep_alive` 30m, 60 s limit.
6. **Checks (code, after the reply; flag, never delete):**
   - every number in the reply appears in a cited passage (reuse the temporal writer's `traced` rule) → else "not traced";
   - every [n] is one of the given passages → else "unknown citation";
   - author/year/journal-style references → "reference not from this evidence";
   - arms named in the reply belong to the asked comparison → else "comparison mismatch";
   - "you should" → flagged.
7. **Fixed lines (code):** personal question (first/second-person + decision words) → "Your own surgeon can tell you what this means for you."; warning symptoms → "If you have these symptoms now, contact your surgeon or hospital straight away."
8. **Store the turn:** question, reply, passages used, check results, timings, mode.

Tapping a citation opens its passage (and, for papers, the reference).

## 6. Gemma's prior and reconciliation (C10, owner only)

- **Prior:** the memory loop notices a running Full run with no prior and asks Gemma (no passages, by design) for its expected answer as JSON: claims, each with any numbers and studies it believes. Stored as `chat_priors` (labelled unverified). No engine change: it reads `runs.question` and status only.
- **Reconciliation:** after the record exists, for each prior claim: retrieve the top passages; Gemma marks it `supported` / `corrected` (with the evidence's version) / `not_in_evidence`; code checks numbers and citations as in §5. Stored with the prior.
- **Scorecard:** per run and overall: share supported / corrected / not in evidence; invented references (prior references not among the run's references).
- Shown only in the owner view. Never retrieved by the chat.

## 7. Owner view (`/chats`, owner only)

- **Conversations:** all, by run, with the asker's role and date; open to read in full.
- **Suggestions:** turns users marked *Suggest for library*: approve, edit in his own words and approve, or reject. Approved items become `owner_qa` passages for the **topic**, cited as "[Owner-reviewed]".
- **Flagged replies:** any check flag or 👎.
- **Prior vs evidence:** per run, the reconciliation and scorecard.

## 8. Screen (run page)

- Third option **Chat** on Full runs with a record; states: getting ready / ready / unavailable (Gemma down) / not available (Quick, no opinion).
- Window: Clinical / Plain switch; three starter questions built by code from the outcomes; reply streams in; citations; flag marks; fixed lines in their own style; 👍 / 👎; *Suggest for library*; footer "Answers come only from this run's evidence. Educational, not medical advice."
- Ivory/Figtree theme; phone width; usable before hydration (form posts work without JS; streaming needs JS).

## 9. Data model (migration `0014_chat.sql`, additive)

- `chat_passages` — `id`, `record_id → evidence_records`, `kind`, `ref_n`, `pmid`, `comparison`, `text`, `fts` (generated), `embedding vector(768)`, `embed_model`, `created_at`. Permanent with the record.
- `topic_qas` — `id`, `topic_id → topics`, `source_turn_id → chat_turns ON DELETE SET NULL`, `question`, `gemma_reply`, `owner_text`, `status` (suggested / approved / rejected), `author_user_id`, `embedding`, `fts`, timestamps. Approved rows are `owner_qa` passages.
- `chat_conversations` — `id`, `run_id → runs ON DELETE CASCADE`, `user_id`, `mode`, `created_at`.
- `chat_turns` — `id`, `conversation_id ON DELETE CASCADE`, `seq`, `question`, `reply`, `status` (done / partial / not_covered / unavailable), `passage_ids BIGINT[]`, `checks JSONB`, `safety_line BOOL`, `personal BOOL`, `feedback SMALLINT NULL`, `suggested BOOL`, `latency_ms`, `created_at`.
- `chat_priors` — `id`, `run_id → runs ON DELETE SET NULL`, `record_id → evidence_records NULL`, `status`, `prior JSONB`, `reconciliation JSONB`, `scorecard JSONB`, timestamps.
- RLS enabled on all, no policies (second lock, as 0013). Retention: conversations and turns go with the run (365 days); passages, approved Q&As and priors linked to a record stay.

## 10. Configuration

`CHAT = off | on` (default off; needs `MEMORY=on`); `CHAT_MODEL` (default `gemma3:4b`); `CHAT_DAILY_LIMIT` (default 30 per user); `CHAT_PRIOR = off | on` (default on when CHAT on). Uses the existing `GEMMA_URL` / `GEMMA_AUTH`. Deploy workflow passes them like the memory variables.

## 11. Limits and failure handling

- No per-message cost (own server). One reply in flight per user; the server queues globally (Ollama serves one at a time); a busy queue → "Gemma is busy, try again in a minute."
- Gemma unreachable → "Chat is unavailable right now"; the answer and Expert Opinion are unaffected. A reply cut at 60 s keeps what was written, marked incomplete.
- Never log question or reply text; the Gemma password stays in Secret Manager.
- Parked from PR A, done here: GEMMA_URL parsed with a real URL parser (no backslash bypass); advisory-lock drop guard; per-record embedding attempt cap.

## 12. Testing

- **Pure:** passage builder on the three fixtures (counts by kind, comparison tags, `ref_n`); retrieval excludes other-comparison passages; checks (numbers, citations, references, comparison, "you should") on scripted replies; safety and personal detection.
- **Integration (fake Gemma via wiremock):** conversation → turns stored; not-covered path never calls Gemma; streaming; limits; owner approval → `owner_qa` retrievable for the topic's other runs; prior and reconciliation flow; retention cascade.
- **Real-Gemma evaluation (bin, owner VPS):** ~20 questions per test run (factual, "how sure", personal, warning symptom, off-topic, other comparison) scored for citations correct, numbers traced, comparison matched, correct "Not covered". Regression: tonight's no-passage TAR question must give "Not covered in this evidence."
- House checks as always; owner tries it on the bilateral TAR run before `CHAT=on` for anyone else.

## 13. Stages

| Stage | Delivers | Visible |
|---|---|---|
| CH0 | Migration 0014, `CHAT` switches, deploy variables, PR A hardening | No |
| CH1 | Passage builder (pure) + prep in the memory job + backfill for existing records | No |
| CH2 | Retrieval, prompt, streaming Gemma client, checks, fixed lines; evaluation bin | No |
| CH3 | Chat API: start, ask, stream, feedback, suggest; limits | No |
| CH4 | Run-page Chat option and window (mockup first) | When on |
| CH5 | Owner view: conversations, suggestions, flagged (mockup first) | Owner |
| CH6 | Prior + reconciliation + scorecard in the owner view | Owner |
| CH7 | Real-Gemma evaluation on the owner's runs; owner smoke; `CHAT=on` | — |

## 14. Open items

- Voice (browser speech) after CH7.
- More CPU on the VPS if replies feel slow in real use.
- Sign-up notice wording for C4 (owner reads chats) — owner to approve before inviting users.
- Prior claims shown to users ("common belief vs evidence") — later, only after the owner trusts the reconciliation.
