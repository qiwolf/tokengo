# tokengo

[中文](README.md) | English | [MIT License](LICENSE)

A self-hosted OpenAI-compatible LLM gateway written in Rust, with an Axum server, SQLite storage and a Leptos WebAssembly administration panel. No Node.js runtime or npm build is required.

## Version and Scope

This public source snapshot is **v0.34.0**. It separates the non-streaming completion deadline from the streaming idle timeout. Recent updates include explicit upstream account-fault isolation, single recovery trials, encrypted custom headers, unknown-usage reporting and live request progress.

Supported client endpoints are chat completions (including SSE), embeddings, reranking and model listing. This is not a full OpenAI API implementation: Responses, native Anthropic/Gemini protocols, audio and video endpoints are not provided. Upstream model capabilities still determine whether tools, reasoning and image input work.

## Features

- Configure upstream URLs, credentials, model aliases and custom request headers.
- Pull upstream model catalogs and manage which models each channel serves.
- Priority routing, retries, failover, recovery, automatic selection and channel concurrency limits.
- Separate non-streaming completion deadlines and streaming inactivity timeouts.
- Route image input only to channels marked as capable; do not silently discard images.
- Named gateway API keys, authenticated administration and encrypted stored upstream secrets.
- Request/attempt accounting, running-request progress, runtime logs and retention controls.
- Track missing usage separately from explicitly reported zero usage.
- Six administration pages: overview, channels, models, requests, logs and settings.

## Build From Source

Requirements: Rust 1.98 or later, a native C compiler/linker for bundled SQLite, the wasm32 target and [Trunk](https://trunkrs.dev/). Use an official Trunk binary if a source installation fails to resolve dependencies. Install toolchain prerequisites for your operating system first.

```sh
git clone https://github.com/shang1626/tokengo.git
cd tokengo
cargo build --locked --release -p tokengo-server
rustup target add wasm32-unknown-unknown
cd crates/web
trunk build --release
cd ../..
```

The server executable is `target/release/tokengo-server`; panel assets are in `crates/web/dist/`. Keep the executable and panel from the same source revision. The public release contains source, not prebuilt production executables or a published container image.

## Run Locally

Run from the repository root:

```sh
TOKENGO_LISTEN=127.0.0.1:3160 \
TOKENGO_DB=./tokengo.db \
TOKENGO_MASTER_KEY_FILE=./master.key \
TOKENGO_WEB_DIR=./crates/web/dist \
TOKENGO_LOG_DIR=./logs \
./target/release/tokengo-server
```

Open http://127.0.0.1:3160/ui/ and initialize your administrator password before making the service public. A persistent master key is created on first startup; the process must be able to write it. Preserve it with the database.

1. Complete first-time setup and store the gateway key securely.
2. Add an upstream in Channels. Supply its OpenAI-compatible base URL and upstream credentials. For a provider whose endpoint is `/v1/chat/completions`, the base URL normally ends in `/v1`, not `/chat/completions`.
3. Add/select the model names served by that channel. Set the upstream model name when it differs from the client-facing name.
4. Set the model purpose (chat, embeddings or rerank) and image capability as appropriate. A name alone is not evidence of capability.
5. In Settings, create named gateway keys for clients. Gateway keys and upstream provider keys are different credentials.
6. Point your client at `http://127.0.0.1:3160/v1`, using a gateway key and a configured model name.

## Environment Variables

| Variable | Default or behavior | Purpose |
| --- | --- | --- |
| TOKENGO_LISTEN | 127.0.0.1:3160 | HTTP bind address |
| TOKENGO_DB | tokengo.db | SQLite database path |
| TOKENGO_MASTER_KEY_FILE | master.key | Persistent encryption key file |
| TOKENGO_WEB_DIR | Auto-discovery | Built panel directory; set explicitly in production |
| TOKENGO_LOG_DIR | ./logs in server entry point | Runtime file log directory |

Relative paths resolve against the working directory. Routing, retention and timeout settings are managed through the panel, not an invented configuration file. Default routing values in this snapshot include 3 retries after the initial attempt, a 30-second attempt/stream-idle timeout and a 120-second non-streaming deadline; saved settings can override them. These are not a guarantee that every request lasts exactly this long.

## Client Examples

Replace the example key and model names with your own. Do not paste an upstream provider key into the gateway key slot.

```sh
export TOKENGO_API_KEY='YOUR_GATEWAY_KEY'
curl http://127.0.0.1:3160/v1/models \
  -H "Authorization: Bearer $TOKENGO_API_KEY"

curl http://127.0.0.1:3160/v1/chat/completions \
  -H "Authorization: Bearer $TOKENGO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-chat-model","messages":[{"role":"user","content":"Hello"}]}'

curl -N http://127.0.0.1:3160/v1/chat/completions \
  -H "Authorization: Bearer $TOKENGO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-chat-model","stream":true,"messages":[{"role":"user","content":"Hello"}]}'

curl http://127.0.0.1:3160/v1/embeddings \
  -H "Authorization: Bearer $TOKENGO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-embedding-model","input":"Example text"}'

curl http://127.0.0.1:3160/v1/rerank \
  -H "Authorization: Bearer $TOKENGO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-rerank-model","query":"Rust gateway","documents":["A Rust gateway","A recipe"],"top_n":1}'
```

| Method | Path | Purpose |
| --- | --- | --- |
| POST | /v1/chat/completions | Chat and SSE streaming |
| POST | /v1/embeddings | Embeddings |
| POST | /v1/rerank | Reranking |
| GET | /v1/models | Configured model list |
| GET | /livez | Process liveness |
| GET | /readyz | Database readiness, not an all-upstreams check |

Once streaming output begins, failures are not retried on another channel: replaying partial tool calls could cause duplicate actions. Client cancellation is recorded separately from upstream failure. Usage that was not reported must not be interpreted as zero cost.

## Production Deployment

Copy the executable and complete panel directory to a persistent location such as `/opt/tokengo`. Use absolute paths, a dedicated unprivileged service account with write access to its data directory, and a service supervisor. An illustrative systemd unit follows; create the account and directories and copy files before enabling it.

```ini
[Unit]
Description=tokengo gateway
After=network-online.target
Wants=network-online.target

[Service]
User=tokengo
Group=tokengo
WorkingDirectory=/opt/tokengo
Environment=TOKENGO_LISTEN=127.0.0.1:3160
Environment=TOKENGO_DB=/opt/tokengo/tokengo.db
Environment=TOKENGO_MASTER_KEY_FILE=/opt/tokengo/master.key
Environment=TOKENGO_WEB_DIR=/opt/tokengo/web
Environment=TOKENGO_LOG_DIR=/opt/tokengo/logs
ExecStart=/opt/tokengo/tokengo-server
Restart=on-failure
RestartSec=3
UMask=0077

[Install]
WantedBy=multi-user.target
```

Place HTTPS in front of the loopback listener. Preserve Authorization headers and disable reverse-proxy response buffering for SSE; give long responses an appropriate proxy read timeout. Restrict the administration surface rather than assuming an API key alone protects every deployment mistake. Initialize locally before exposing the service. This unit is a generic example, not an automatically installed deployment package.

## Backup, Upgrade and Recovery

Back up the database and matching master key together. For a running WAL database use SQLite's backup API; do not copy only the `.db` file while writes continue. Alternatively stop the service cleanly before copying its persistent data. Protect backups as credentials. Losing the key prevents decrypting stored secrets.

Before upgrading, save a complete backup of database, key, executable, panel and service configuration. Build and test a fixed revision, then replace the executable and panel together. Startup applies database migrations. Verify `/api/ping`, `/readyz`, the panel and a real upstream request. Do not assume an older executable can read a migrated database: rollback may require the matching full backup and loses writes since that backup. Test restoration on a separate instance first.

## Tests and Verification

```sh
cargo test --locked --workspace
cargo test --locked --workspace -- --test-threads=1
cargo check -p tokengo-web --target wasm32-unknown-unknown
```

For the v0.34.0 public snapshot, serial workspace tests passed **403 tests, with 1 ignored**. The initial parallel run failed one running-request timing assertion; its isolated rerun and the serial workspace run passed. The parallel failure has not been conclusively diagnosed. These results do not certify every provider, browser or deployment. Unset proxy environment variables when running local HTTP fixtures if your proxy intercepts loopback traffic.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Blank or unavailable panel | Build with Trunk; point TOKENGO_WEB_DIR at the complete dist directory; check browser asset errors |
| 401 on /v1/* | Use a current gateway key, not the panel password or provider key |
| Model not found / 404 | Verify the exact client model name is selected on an enabled channel |
| Image request rejected / 422 | Confirm at least one eligible channel supports image input; the image is not silently stripped |
| 429, 502 or 503 | Inspect request attempts and runtime warnings; distinguish provider quota, credentials, network and concurrency |
| Long request stops | Check non-streaming deadline, stream inactivity and reverse-proxy timeouts separately |
| Key file cannot be written | Check ownership, permissions and absolute data paths; never replace an existing key casually |
| Missing token usage | Upstream may not report it; unknown does not mean zero |

## Source, Security and Contributing

- `crates/server`: Axum HTTP server, SQLite, credentials, accounting and routing integration.
- `crates/shared`: shared data structures and routing state machine.
- `crates/web`: Leptos panel, HTML and CSS, compiled to WebAssembly.
- `crates/cli`: routing command-line demonstration.

The public repository preserves sanitized history. Private operations reports, internal deployment scripts, databases, logs, secrets and build output are deliberately excluded. Old README snapshots describe old versions. No current performance/memory guarantee is inferred from historical measurements.

Report bugs through GitHub Issues with version, reproducible steps and redacted errors. Never post keys, passwords, database dumps or sensitive request content. For changes, explain behavior and add a regression test; run workspace tests and the WASM check for panel changes. There is no dedicated private security-reporting address advertised here: do not publish exploit details containing credentials in a public issue.

## License and Attribution

MIT; see [LICENSE](LICENSE) and [NOTICE](NOTICE). FreeLLMAPI informed compatibility behavior. Only connect upstream services you are authorized to use and respect their terms and quotas.
