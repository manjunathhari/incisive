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
