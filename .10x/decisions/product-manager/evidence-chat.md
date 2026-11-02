# Product Manager — evidence-chat

Goal (owner, 2 Nov 2026): a "Chat" option next to Answer | Expert opinion on the run page. The user chats conversationally within the context of that run's answer and Expert Opinion; Gemma answers only from them (retrieval over a vectorised knowledge base of the run), never introducing outside data. Conversations are stored; no model training.

## Decisions
- C1 Audience: either surgeons or patients; one conversational chat with a Clinical / Plain language switch (2 Nov).
- C2 Order: chat first, then Topics (PR B), then gate (PR C). The three parked Gemma hardening items fold into the chat build (2 Nov).
- C3 Text first, voice next (2 Nov).

## Evidence from the VPS test (2 Nov, gemma3:4b, 2 CPU)
- Warm answer ≈ 15 s (prompt 268 tokens at 42.7 tok/s ≈ 6 s; 103 tokens out at 11.8 tok/s ≈ 9 s); cold load +8.6 s → keep_alive.
- Grounding: 2 of 3 sentences faithful with citations and uncertainty kept; sentence 3 used a robotic-vs-laparoscopic passage for a robotic-vs-open question; jargon ("TAR") left unexplained.
- Design consequences: 2–3 most relevant passages only; prompt names the comparison; plain-language rules; code checks (numbers traced, citations real, comparison match).
- C4 Visibility (2 Nov): each conversation is private to its user (no other user sees it); the owner can read all conversations. Sign-up notice must say the owner can read chats. Chat replies join the shared knowledge base only after owner approval ("Suggest for library" → owner approves/edits in his own words). Chats follow the 12-month rule like runs.
- C5 Scope (2 Nov): this run only — the run's answer + Expert Opinion, plus owner-approved Q&As for its topic. No other runs, no whole-library retrieval in v1.
- C6 Eligibility (2 Nov): Chat option only on Full runs with a finished Expert Opinion (i.e. runs with a memory record). Quick runs: no chat in v1.
- C7 Personal questions (2 Nov): answer from the evidence for people in that situation, always end "Your own surgeon can tell you what this means for you", never "you should"; code (not Gemma) adds a fixed safety line when warning symptoms are mentioned (fever, severe pain, bleeding…) → contact your surgeon or hospital now.
- Design Section 1 (how a question works) accepted 2 Nov.
- C8 No-passage rule (2 Nov, after the no-context test: gemma3:4b invented 3 references — none in PubMed — and stay figures, and dropped all uncertainty): Gemma is never called without passages; no relevant passage → code replies "Not covered in this evidence". Code flags any citation not among the given passages, any number not in a cited passage ("not traced"), and any author/year/journal-style reference. That question with no passages is a regression test.
- Design Section 2 (screen, owner view, limits) presented 2 Nov; owner then added:
- C9 Run evidence in the knowledge base (2 Nov): include the selected papers' abstracts (copied from paper_cache, kept permanently with the record), verified quotes and reference details; prepared as the answer is saved, opinion passages added when it lands — chat ready when the run is.
- C10 Gemma prior + reconciliation (2 Nov): during the run Gemma writes its prior (File 1, "unverified", snapshot); after the run each prior claim is marked supported / corrected / not in evidence against the run evidence (File 3; numbers checked by code). The chat never uses File 1. File 3 ("common belief vs evidence") is OWNER VIEW ONLY for now, plus a per-run scorecard of Gemma's prior accuracy.
