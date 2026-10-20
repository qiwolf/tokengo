# syntax=docker/dockerfile:1
# Build the Rust server/CLI and WASM panel from this one source tree.
FROM rust:1.98.0-bookworm AS build
RUN apt-get update && apt-get install --no-install-recommends -y \
    build-essential pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown \
    && cargo install --locked --version 0.21.14 trunk
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
ENV NO_COLOR=true
RUN cargo build --locked --release -p tokengo-server -p tokengo-cli \
    && cd crates/web && trunk build --locked --release
ARG VCS_REF=unknown
RUN mkdir -p /out/bin /out/web \
    && cp target/release/tokengo-server target/release/tokengo /out/bin/ \
    && cp -r crates/web/dist/. /out/web/ \
    && printf '{"commit":"%s","source":"Dockerfile source build"}\n' "$VCS_REF" > /out/manifest.json

FROM ubuntu:24.04 AS runtime-base
RUN apt-get update && apt-get install --no-install-recommends -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 tokengo \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /data --shell /usr/sbin/nologin tokengo \
    && mkdir -p /data/logs /opt/tokengo \
    && chown -R 10001:10001 /data && chmod 0700 /data /data/logs
COPY LICENSE NOTICE /opt/tokengo/
COPY deploy/container/healthcheck.sh /usr/local/bin/tokengo-healthcheck
ENV TOKENGO_LISTEN=0.0.0.0:3160 \
    TOKENGO_DB=/data/tokengo.db \
    TOKENGO_MASTER_KEY_FILE=/data/master.key \
    TOKENGO_WEB_DIR=/opt/tokengo/web \
    TOKENGO_LOG_DIR=/data/logs
WORKDIR /data
USER 10001:10001
EXPOSE 3160
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["/usr/local/bin/tokengo-healthcheck"]
ENTRYPOINT ["/opt/tokengo/bin/tokengo-server"]

# CI can reuse the verified same-commit Linux release bundle instead of rebuilding.
# The context must contain bundle/{bin,web,manifest.json}; only these are copied.
FROM runtime-base AS release-runtime
COPY bundle/bin/ /opt/tokengo/bin/
COPY bundle/web/ /opt/tokengo/web/
COPY bundle/manifest.json /opt/tokengo/manifest.json
ARG VCS_REF=unknown
LABEL org.opencontainers.image.source="https://github.com/qiwolf/tokengo" \
    org.opencontainers.image.revision="$VCS_REF"

# Default target: build both halves from source.
FROM runtime-base AS runtime
COPY --from=build /out/ /opt/tokengo/
ARG VCS_REF=unknown
LABEL org.opencontainers.image.source="https://github.com/qiwolf/tokengo" \
    org.opencontainers.image.revision="$VCS_REF"
