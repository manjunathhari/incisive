# Architect — memory-layer (in progress)

- **A1 Embedding host (2026-11-01, owner):** owner's Hostinger VPS, Mumbai (mum2), 2 CPU: Ollama in Docker (container `ollama-s9os-ollama-1`), Traefik present. Model `embeddinggemma` (to be pulled; gemma3:4b already installed for later chat).
  - Access: HTTPS subdomain via Traefik + basic auth; public port 32768 to be closed. Credential in Secret Manager `gemma-auth`.
  - Embeddings computed asynchronously after save (never on the run's critical path). Query-time embedding for the gate/search has a short timeout; on failure search falls back to Postgres full-text only.
  - embeddinggemma returns 768-dim vectors (verified on the VPS 1 Nov 2026). Vectors are tied to model name + dimension stored with each row; a model change means a re-index.
  - Endpoint name: srv2117610.hstgr.cloud (VPS hostname; no new DNS). Ollama port 32768 currently public on IPv4+IPv6 — to be removed when routed through Traefik.
- **A2 Gemma endpoint live (2026-11-01, owner on VPS):** `https://ollama-s9os.srv2117610.hstgr.cloud`, Traefik + Let's Encrypt, basic auth user `incisive` (bcrypt hash in /docker/ollama-s9os/.env as OLLAMA_AUTH), router limited to Path /api/embed and /api/chat. Host port mapping removed (container shows 11434/tcp only). Verified: 401 without auth, vector with auth. Password held by owner; to go into Secret Manager as `gemma-auth`. Backup of Hostinger's original compose at docker-compose.yml.bak. Caution: editing the app in Hostinger's Docker panel may restore the template.
