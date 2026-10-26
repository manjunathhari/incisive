# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /build
# rust-toolchain.toml selects "stable", which is not the image's pinned default toolchain;
# add the wasm target from inside /build so it lands on the toolchain the build actually uses.
COPY rust-toolchain.toml .
RUN rustup show active-toolchain || rustup toolchain install \
 && rustup target add wasm32-unknown-unknown \
 && cargo install cargo-leptos --version 0.3.11 --locked
COPY . .
RUN cargo leptos build --release

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
