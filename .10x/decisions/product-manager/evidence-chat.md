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
