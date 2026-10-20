# tokengo

[中文](README.md) | English | [MIT License](LICENSE)

A self-hosted OpenAI-compatible LLM gateway written in Rust, with an Axum server, SQLite storage and a Leptos WebAssembly administration panel. No Node.js runtime or npm build is required.

## Version and Scope

This fork builds on upstream **v0.34.0** and retains that Cargo version. Use the commit identity and current documentation to determine behavior; build the server and panel from the same commit.

The fork provides native Chat, Responses and Messages endpoints, strict text/client-function conversion in all six directions, embeddings, reranking, model listing and standalone DeepSeek Search/Fetch. Users, scoped virtual keys, ancestor quotas, ordered fallback, versioned prices, budgets and durable per-attempt accounting share one executor. Configure source capabilities explicitly; a model name is not evidence of support.

See the [changelog](CHANGELOG.md), [provider channels](docs/provider-channels.md), [accounts and self-service Keys](docs/account-login.md), [protocols and clients](docs/protocols.md), [identity and metering](docs/identity-metering.md), [routing](docs/logical-routing.md), [Search/Fetch](docs/deepseek-search-fetch.md), [independent OpenAI OAuth](docs/openai-oauth.md), [requirements coverage](docs/requirements-status.md), and [deployment](docs/deployment.md). Implementation and local tests do not establish acceptance by every real provider, subscription or CLI.

Conversion rejects semantics it cannot represent. Responses requires full input replay; native signatures and opaque state have ownership restrictions. Native Chat inline audio is an explicit source capability, but separate audio/video/file APIs, native Gemini and arbitrary subscription conversion are not implemented. The official OAuth preview rejects some ordinary API generation fields; review its specific limitations.

This branch implements these features; production acceptance is tracked in the [public acceptance status and limits](docs/acceptance-status.md). The Cargo version remains **0.34.0**. No new release or tag is implied by this README.

## Provider Channels and Native Parameters

Channels now presents an explicit provider identity above its connections. Connections retain their original IDs, URLs, credentials, protocols, OAuth bindings, concurrency, pricing and fallback references. The current grouping plan organizes existing sources into six providers; this is a configurable deployment arrangement, not a hard-coded provider catalog or limit. Assign ungrouped connections explicitly rather than inferring identity from hostnames or model names.

A provider's model switch atomically updates the explicitly listed connections. Disabling preserves aliases and capability configuration. The Models page defaults to public, enabled, routable models; disabled and compatibility-hidden items can be restored through explicit filters. Provider, connection, per-connection model and global logical-model switches have distinct scopes.

Reasoning is a native parameter: `reasoning.effort` for Responses or `reasoning_effort` for Chat, independent of `service_tier`. Explicit client values take precedence. Adding/editing models reads a normal upstream catalog or the bound OAuth account's official catalog without paid inference. Levels, defaults and provenance follow actual metadata. A reasoning boolean does not establish effort levels; unknown remains unknown, with explicit manual confirmation available and labeled as such. Catalog declarations are not inference tests; cross-protocol support depends on the configured protocol.

The standard GPT display names are `gpt-6.1-sol`, `gpt-6-luna` and `gpt-5.6-luna`, subject to the current account's catalog and access. Effort levels are not separate models. Ten existing effort aliases can remain as explicitly hidden compatibility entries: excluded from `/v1/models` and auto, while direct requests, history and internal references remain intact. No suffix-based renaming or deletion is performed.

For example, send the following to `POST /v1/responses` only if the current account catalog offers this model and effort. A supported `service_tier` is a separate top-level field, not inferred from reasoning effort.

```json
{"model":"gpt-6-luna","stream":true,"store":false,"reasoning":{"effort":"low"},"input":[{"role":"user","content":"Hello"}]}
```

## Accounts, Keys and Observability

Login uses a username, password and same-origin HttpOnly cookie. The initial root administrator is `admin`; changing its name or password requires the current password and revokes sessions. The unified Users page creates an ordinary user, login credentials and permissions together. Existing users can gain login without changing Key or ledger ownership. Owners manage their own Keys and usage after browser login; disabling a user blocks both login and API calls. Root retains its independent security identity.

Multiple VKeys per user support model/connection/paid-source permissions, quotas and ancestor restrictions. Ordered fallback respects those controls and budgets. A stream may fail over before output is committed; committed output is never replayed on another source. See [identity and metering](docs/identity-metering.md) and [logical routing](docs/logical-routing.md).

The overview shows real request metrics, active progress and grouping by type, connection, model and client. Billing exposes immutable prices, budgets and per-attempt ledgers. The current dashboard emphasizes metrics and detail lists; it is not a complete cost-trend charting or general BI suite. Token and request-unit billing remain separate, and missing usage or unpriced costs are not displayed as known zero.

## Verification Boundaries

Native protocol support is not semantic equivalence across providers. Text/client-function conversion and representable plaintext reasoning/request fields are limited subsets; signatures, encrypted reasoning, opaque state and role differences remain constrained. See [protocols](docs/protocols.md), including DeepSeek's native compatibility differences and the OAuth preview's field restrictions.

Standalone DeepSeek Search/Fetch has limited real-request and request-unit accounting verification. It does not automatically implement CLI hosted search, and configured ledger prices do not replace the provider's final invoice. Independent OpenAI OAuth has actual authorization, catalog, limited text, controlled single-tool CLI and specific converted-stream verification. Those results do not certify every account, model, effort, parameter or all Claude Code/Codex features. This provider/effort UI update uses isolated browser fixtures and catalog metadata, with no additional paid inference.

## Features

- Configure upstream URLs, credentials, model aliases and custom request headers.
- Explicit provider grouping, independent connections, catalog metadata and non-destructive model switches.
- Priority routing, retries, failover, recovery, automatic selection and channel concurrency limits.
- Separate non-streaming completion deadlines and streaming inactivity timeouts.
- Route image input only to channels marked as capable; do not silently discard images.
- Unified Users, point-and-click user/Key policies, browser self-service Keys, cookie login and encrypted upstream secrets.
- Request/attempt accounting, running-request progress, runtime logs and retention controls.
- Track missing usage separately from explicitly reported zero usage.
- Administrator console and scoped user portal, with responsive light/dark layouts and a separate diagnostic log entry.
- User/key/ancestor permissions and quotas, immutable token/request prices, durable settlement and unknown-cost reporting.
- Independent OAuth enrollment and rotating encrypted credentials; standalone Search/Fetch with request-unit accounting.

## Build From Source

Use `main` for the current source and pin a reviewed commit for deployment rather than relying on the unchanged Cargo version. Merging source and upgrading production are separate acceptance steps.

Requirements: Rust 1.98 or later, a native C compiler/linker for bundled SQLite, the wasm32 target and [Trunk](https://trunkrs.dev/). Use an official Trunk binary if a source installation fails to resolve dependencies. Install toolchain prerequisites for your operating system first.

```sh
git clone --branch main https://github.com/qiwolf/tokengo.git
cd tokengo
cargo build --locked --release -p tokengo-server
rustup target add wasm32-unknown-unknown
cd crates/web
trunk build --release
cd ../..
```

The server executable is `target/release/tokengo-server`; panel assets are in `crates/web/dist/`. Keep the executable and panel from the same source revision. See [GitHub Releases](https://github.com/qiwolf/tokengo/releases) for the actual published native and offline-container assets. The Linux packaging CI also retains a same-commit offline image artifact for 14 days. No public container registry is published automatically. See the [container deployment guide](docs/container-deployment.md) for the multi-stage Dockerfile, non-root Compose setup, persistent data, backup and offline distribution. No production accounts or credentials are included.

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

1. Complete first-time setup and safely save the once-displayed gateway key. An omitted display name uses the administrator default.
2. Create a provider in Channels. Explicitly assign old connections or add a connection under its advanced settings; its name identifies a connection, not a second provider. Set the URL for its actual native protocol and supply credentials.
3. Fetch catalog metadata and enable the desired standard model names. Configure upstream aliases when needed. The provider, connection and model must all be enabled.
4. Configure purpose, input capability and native defaults only from actual declarations or explicit confirmation; unknown capability is not guessed from a name.
5. In Users, create a login name, display name and initial password, select model/connection permissions and limits, and optionally set a monetary cap. Configure real source prices before using monetary budgets. Enable login within an existing user when needed. Ordinary users then sign in to My Usage and Keys to create their own Keys within their user permissions and budget.
6. Point the client at `http://127.0.0.1:3160/v1` with a VKey and an allowed configured model name.

## Environment Variables

| Variable | Default or behavior | Purpose |
| --- | --- | --- |
| TOKENGO_LISTEN | 127.0.0.1:3160 | HTTP bind address |
| TOKENGO_DB | tokengo.db | SQLite database path |
| TOKENGO_MASTER_KEY_FILE | master.key | Persistent encryption key file |
| TOKENGO_WEB_DIR | Auto-discovery | Built panel directory; set explicitly in production |
| TOKENGO_LOG_DIR | ./logs in server entry point | Runtime file log directory |
| TOKENGO_OPENAI_PROXY | Default HTTP client behavior when unset | Dedicated OpenAI OAuth/subscription proxy; does not change ordinary source clients |

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
| POST | /v1/responses | Native Responses or an explicitly permitted conversion |
| POST | /v1/messages | Native Messages or an explicitly permitted conversion |
| POST | /v1/tools/search | Standalone DeepSeek Search |
| POST | /v1/tools/fetch | Standalone DeepSeek Fetch |
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
cargo check --locked -p tokengo-web --target wasm32-unknown-unknown
```

Tests cover local HTTP/SSE fixtures, permissions, accounting, recovery, protocols and frontend logic. Passing fixtures or isolated browser acceptance does not certify all real providers or a deployment. Check the exact commit's CI and [public acceptance status](docs/acceptance-status.md) instead of treating a historical test count as current proof. Unset proxy environment variables if a proxy intercepts loopback fixtures.

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

## Protocol matrix and next phases

See the [protocol routing matrix and fallback diagram](docs/protocol-routing-matrix.md) for native selection, bounded bridging and explicit rejection. The first unified-user phase implements atomic user/login creation, browser self-service Keys and point-and-click policies. It adds no schema, user expiry or self-service toggle; browser self-service creation has a fixed 32-active-Key cap. Monetary caps are not raw Token packages. Monthly entitlements, cumulative Token quota and custom probes remain future work. See [requirements coverage](docs/requirements-status.md), the [phased plan](docs/unified-user-and-quota-plan.md) and [acceptance status](docs/acceptance-status.md) for implementation and verification boundaries.


## Provider Key pools and private virtual models

The fork now models a provider channel as one API base URL with multiple upstream credentials and models. Ordered or concurrent allocation preserves user affinity across downstream VKeys, with explicit 429/400 switches. Users can create private virtual model names with ordered targets or weekly schedules (UTC or Asia/Shanghai); every attempt rechecks real target permissions, pricing and budget. Native session pins take precedence and committed streams are never replayed. Multi-target embeddings remain rejected until dimensions are verified. See the [behavior contract and flow diagram](docs/provider-key-pools.md) and [acceptance boundaries](docs/acceptance-status.md); source implementation, CI artifacts, Releases and production deployment are verified separately.
