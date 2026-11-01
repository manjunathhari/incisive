# Product Manager — memory-layer

## Decisions (owner)
- **D1 Retention (2026-11-01, owner chose A):** a user's private run history keeps the 12-month delete. On save, a de-identified copy of the evidence pair (Full answer + Expert Opinion) goes into a permanent shared evidence library with no link to who asked. Invitees are told at sign-up.

## Open
- D2 Topic grouping: how different wordings of one question land on one topic.
- **D2 Topic grouping (2026-11-01, owner):** fully automatic by code. Store the framed PICO; code matches the question to the closest existing topic (intervention, comparator, population) or creates a new one. No user confirmation step.
- **D3 Owner admin (2026-11-01, owner):** owner can merge and split topics afterwards; every merge/split logged and reversible. (Mitigates D2's silent wrong merges.)
- **D4 Gate (2026-11-01, owner):** when the topic already has a stored Full run, open the stored answer + Expert Opinion at once (no cost) and offer "Update with newer evidence", a fresh Full run that searches only papers after the stored run's date.
- **D5 Shared library visibility (2026-11-01, owner):** only the owner sees the shared (de-identified, all-users) library. Other users see only their own runs.
- **D6 Gate scope for other users (2026-11-01, owner):** every user's gate matches against all stored runs, de-identified (they can open a stored answer from anyone's run, but cannot browse the shared library and never see who asked).
- **D7 Expert layer (2026-11-01, owner):** structured verdict (adopt / selective / not yet) plus a note, per topic, dated and versioned, owner-written.
- **D8 Search (2026-11-01, owner):** full-text and meaning-search (embeddings + pgvector) both in v1.
- **D9 Update in v1 (2026-11-01, owner chose A):** "Update with newer evidence" runs a normal Full run; the engine stays frozen. The new record joins the topic and the ledger merges studies (duplicates by PMID counted once). Date-limited search deferred to its own approved engine change.
- **Design Section 1 (tables + flows) accepted 2026-11-01.**
