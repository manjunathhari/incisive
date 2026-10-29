# Stage 0: faster releases (branch stage0-fast-builds, not pushed)

## What changed
- Dockerfile: stages base (toolchain, cargo-leptos 0.3.11, cargo-chef 0.1.78 pinned) -> planner (`chef prepare`) -> builder. Before `COPY . .` the builder cooks:
  - server: `--release --no-default-features --features ssr -p incisive-app --bin incisive-app`
  - wasm: `--profile wasm-release --target wasm32-unknown-unknown --no-default-features --features hydrate -p incisive-app --lib`
  Runtime stage, ENV and ENTRYPOINT unchanged.
- ci.yml container job: setup-buildx + build-push-action (load, gha cache scope incisive, from and to). Smoke test unchanged. No new permissions (default token suffices).
- deploy.yml: setup-buildx + build-push-action (push, tag from env IMAGE, cache-from only) replaces docker build and docker push. All check steps, env-file logic and gcloud flags untouched.
- infra/README.md: "Build caching" subsection.

## Deviation
The old build step tee'd the log and annotated its last 80 lines. build-push-action has no such log, so that tail annotation is gone; the action raises its own error annotation with buildx's error output. If the full tail matters, switch back to a `docker buildx build` run step with the same flags.

## Expected timings (estimates, unverified)
Cold build unchanged (~20+ min). Warm, dependencies unchanged: CI container and deploy build each ~3-6 min (app crate compile for ssr and wasm, plus wasm-bindgen/wasm-opt). Deploy hits the cache written by CI on main.

## Risks
- Cache misses: any Cargo.toml/Cargo.lock change rebuilds the cook layer. cargo-leptos may set env/RUSTFLAGS that differ from the cook, which would recompile some deps (still correct, just slower); not verifiable here. cargo-leptos also downloads wasm-bindgen/wasm-opt at build time, not cached in a layer.
- PR branches cannot read each other's caches; only main and the branch itself. First main build after merge is cold if main never wrote the cache.
- 10 GB per repo cap, LRU eviction; mode=max with a Rust image can be several GB, and the cache can evict the Swatinem rust-cache entries. Clear via Actions > Caches.
- Buildx credentials: setup-buildx uses the docker-container driver; auth is sent from the client's ~/.docker/config.json (written by `gcloud auth configure-docker`), which is the standard documented flow, but only a real run proves it. Fallback: docker/login-action with the gcloud access token (adds a step).
- cargo chef cook with `-p`/`--lib`/`--bin` flags and cargo-chef 0.1.78 could not be run here (no Docker); the flag combination is from its documented CLI.

## Validation run
YAML parse of both workflows; `bash -n` on every run script; `cargo test -p incisive-app --test runs deploy` and `unpaywall` (pass, no test edits needed); cargo fmt --check; clippy -D warnings (clean). Docker, Actions and cargo-chef were not run.
