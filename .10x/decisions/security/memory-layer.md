# Security — memory-layer PR A

- GEMMA_AUTH: SecretString, manual Debug, never logged or in URLs; basic auth header marked sensitive; redirects disabled; https required except loopback. Parked: backslash in authority bypasses the loopback check (operator-set var; fix PR B).
- No question/answer/board text in logs or error codes; backfill bin prints count only.
- SQL: all user text bound; FTS words reduced to alphanumerics and bound.
- VPS: Ollama port closed; Traefik basic auth; only /api/embed and /api/chat routed (verified 1 Nov).
- RLS enabled on new tables as a second lock (Data API off).
