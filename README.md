# tokengo

Self-hosted OpenAI-compatible LLM gateway written in Rust, with a Rust/WASM administration panel.

## Current Version

v0.30.1 includes independent running-request status and live upstream channel/model progress. Completed requests cannot be reverted to running by delayed accounting updates; interrupted requests are recovered at startup. The panel refreshes expanded request details alongside the recent-call list.

Recent updates also include model/channel cooldown visibility, configurable model context limits, upstream capability metadata and named gateway API keys.

## Features

- Chat completions and SSE streaming, embeddings, reranking and model discovery.
- Custom upstream channels, model aliases and templated request headers.
- Priority routing, retries, failover, recovery and per-channel concurrency limits.
- Request accounting, runtime logs, model capability metadata and image-input routing.
- SQLite storage, encrypted upstream credentials and authenticated administration.
- Leptos web panel with no npm build dependency.

## Build

Requires Rust 1.98 or newer and a native C toolchain for bundled SQLite.
Install Trunk using the official instructions at https://trunkrs.dev/ for the web panel.

```sh
cargo build --locked --release -p tokengo-server
cargo test --locked --workspace
rustup target add wasm32-unknown-unknown
cd crates/web
trunk build --release
cd ../..
```

## Run

From the repository root after building the server and panel:

```sh
TOKENGO_LISTEN=127.0.0.1:3160 \
TOKENGO_DB=./tokengo.db \
TOKENGO_MASTER_KEY_FILE=./master.key \
TOKENGO_WEB_DIR=./crates/web/dist \
TOKENGO_LOG_DIR=./logs \
./target/release/tokengo-server
```

Open http://127.0.0.1:3160/ui/ and complete first-time setup. Set your own panel password and gateway API key. Add upstream channels and credentials, then select the models each channel serves. Clients use the gateway key, not an upstream key.

```sh
curl http://127.0.0.1:3160/v1/models \
  -H 'Authorization: Bearer YOUR_GATEWAY_API_KEY'
```

| Method | Path | Purpose |
| --- | --- | --- |
| POST | /v1/chat/completions | Chat and streaming chat |
| POST | /v1/embeddings | Embeddings |
| POST | /v1/rerank | Reranking |
| GET | /v1/models | Configured model list |
| GET | /livez | Liveness |
| GET | /readyz | Database readiness |

For deployment, copy the release executable and panel files to your server. Set a persistent working directory and the environment variables above. Bind to loopback behind an HTTPS reverse proxy; initialize the administrator account before exposing the service. Use a supervisor such as systemd for automatic restarts.

Back up SQLite using a consistent SQLite backup, together with master.key. Losing the master key makes stored upstream credentials unreadable. Never commit the database, key file, logs or real credentials. Logs can contain request metadata; restrict access and choose retention settings appropriately.

## Source Layout

- crates/server: Axum gateway, storage, accounting and administration API.
- crates/shared: Shared types and routing logic.
- crates/web: Leptos panel compiled to WebAssembly.
- crates/cli: Routing command-line demonstration.

This public repository retains sanitized development history. Private deployment reports, machine-specific scripts and operational data are intentionally excluded. Historical README snapshots describe earlier versions, not current setup instructions. No production binaries or credentials are distributed here.

## License

MIT. See LICENSE and NOTICE for attribution, including FreeLLMAPI, which informed compatibility behavior.
