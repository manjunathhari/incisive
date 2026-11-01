# Product Manager — memory-layer

## Decisions (owner)
- **D1 Retention (2026-11-01, owner chose A):** a user's private run history keeps the 12-month delete. On save, a de-identified copy of the evidence pair (Full answer + Expert Opinion) goes into a permanent shared evidence library with no link to who asked. Invitees are told at sign-up.

## Open
- D2 Topic grouping: how different wordings of one question land on one topic.
- **D2 Topic grouping (2026-11-01, owner):** fully automatic by code. Store the framed PICO; code matches the question to the closest existing topic (intervention, comparator, population) or creates a new one. No user confirmation step.
- **D3 Owner admin (2026-11-01, owner):** owner can merge and split topics afterwards; every merge/split logged and reversible. (Mitigates D2's silent wrong merges.)
- **D4 Gate (2026-11-01, owner):** when the topic already has a stored Full run, open the stored answer + Expert Opinion at once (no cost) and offer "Update with newer evidence", a fresh Full run that searches only papers after the stored run's date.
- **D5 Shared library visibility (2026-11-01, owner):** only the owner sees the shared (de-identified, all-users) library. Other users see only their own runs.
