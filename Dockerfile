# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS base
WORKDIR /build
# rust-toolchain.toml selects "stable", which is not the image's pinned default toolchain;
# add the wasm target from inside /build so it lands on the toolchain the build actually uses.
COPY rust-toolchain.toml .
RUN rustup show active-toolchain || rustup toolchain install \
 && rustup target add wasm32-unknown-unknown \
 && cargo install cargo-leptos --version 0.3.11 --locked \
 && cargo install cargo-chef --version 0.1.78 --locked

# Planner: reduces the workspace to recipe.json (manifests and lockfile only),
# which stays byte-identical until a dependency changes.
FROM base AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Builder: compiles dependencies in a layer keyed on recipe.json, then the app.
FROM base AS builder
COPY --from=planner /build/recipe.json recipe.json
# Same features and profiles cargo-leptos uses (see [package.metadata.leptos]):
# server binary = release + ssr; browser lib = wasm-release + hydrate on wasm32.
RUN cargo chef cook --release --no-default-features --features ssr \
      -p incisive-app --bin incisive-app --recipe-path recipe.json \
 && cargo chef cook --profile wasm-release --target wasm32-unknown-unknown \
      --no-default-features --features hydrate \
      -p incisive-app --recipe-path recipe.json
COPY . .
# The cooked stubs must never be mistaken for the real sources: mark every
# crate source as newer than the cooked output before the real build.
RUN find crates -name '*.rs' -exec touch {} + \
 && cargo leptos build --release

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /build/target/release/incisive-app /app/incisive-app
COPY --from=builder /build/target/site /app/site
ENV APP_ENV=prod \
    LEPTOS_ENV=PROD \
    LEPTOS_OUTPUT_NAME=incisive-app \
    LEPTOS_SITE_ROOT=site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:8080
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/app/incisive-app"]
