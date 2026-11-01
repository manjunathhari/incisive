# Architect — memory-layer (in progress)

- **A1 Embedding host (2026-11-01, owner):** owner's Hostinger VPS, Mumbai (mum2), 2 CPU: Ollama in Docker (container `ollama-s9os-ollama-1`), Traefik present. Model `embeddinggemma` (to be pulled; gemma3:4b already installed for later chat).
  - Access: HTTPS subdomain via Traefik + basic auth; public port 32768 to be closed. Credential in Secret Manager `gemma-auth`.
  - Embeddings computed asynchronously after save (never on the run's critical path). Query-time embedding for the gate/search has a short timeout; on failure search falls back to Postgres full-text only.
  - embeddinggemma returns 768-dim vectors (verified on the VPS 1 Nov 2026). Vectors are tied to model name + dimension stored with each row; a model change means a re-index.
  - Endpoint name: srv2117610.hstgr.cloud (VPS hostname; no new DNS). Ollama port 32768 currently public on IPv4+IPv6 — to be removed when routed through Traefik.
- **A2 Gemma endpoint live (2026-11-01, owner on VPS):** `https://ollama-s9os.srv2117610.hstgr.cloud`, Traefik + Let's Encrypt, basic auth user `incisive` (bcrypt hash in /docker/ollama-s9os/.env as OLLAMA_AUTH), router limited to Path /api/embed and /api/chat. Host port mapping removed (container shows 11434/tcp only). Verified: 401 without auth, vector with auth. Password held by owner; to go into Secret Manager as `gemma-auth`. Backup of Hostinger's original compose at docker-compose.yml.bak. Caution: editing the app in Hostinger's Docker panel may restore the template.
- **A3 Topic key:** the team engine does not frame a PICO (only the classic pipeline did). Topic matching uses the Expert Opinion `board.topic` label + intervention–comparator pairs from the era table + meaning-vectors; no extra LLM call.
- **A4 Tables (migration 0013, additive):** topics, evidence_records (no cascade from runs; source_run_id SET NULL), ledger_studies, ledger_claims, topic_verdicts (append-only), topic_changes (merge/split log).
- **A5 Memory job:** after the temporal job, post-run, can never fail or alter a run. Embeddings filled asynchronously; FTS fallback.

## Design (Phase 2, accepted with ADR-001)
- Module `src/memory/`: settings, extract (pure), matcher (pure), store (SQL), job (queue + loop), embed (HTTP), topics (owner domain), gate.
- Trigger: `runs/temporal.rs` after `finish(.., Done)` → `enqueue_job` (only if MEMORY on). Loop every 30 s claims 5 jobs with FOR UPDATE SKIP LOCKED (Cloud Run up to 3 instances).
- Score: with embeddings 0.5·cos + 0.3·pair-Jaccard + 0.2·FTS; without 0.6·Jaccard + 0.4·FTS. Defaults topic 0.80, gate 0.85 until M6 calibration.
- Failure modes: VPS down → NULL embeddings, sweep later; run deleted mid-job → `run_gone`; non-V3 body → skipped with code; merged chains resolved (≤16 hops).
- Spec amended: `evidence_records.topic_label`, `fts_extra` columns feed the generated `fts`.
