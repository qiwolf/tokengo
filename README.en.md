# tokengo

The current public release is **[v0.34.0-qiwolf.12](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.12)**, source `6bf36fa917cd` (Cargo 0.34.0, schema 40): targeted routing reads with legacy-name compatibility and live authorization, SiliconFlow BGE-M3 native-dimension adaptation, and management layout improvements. Exact-source CI and independent public native/container acceptance passed. CI ran the full Rust workspace (1160 passed, 0 failed, 3 ignored); this build-test round ran 125 Web and 22 Python checks, reusing byte-identical backend evidence rather than rerunning the whole workspace. See the [release notes](docs/releases/v0.34.0-qiwolf.12.md) and [receipt](docs/acceptance/2026-10-24-qiwolf12-release/README.md). Production was not upgraded and still runs the private 0d0 performance patch. Tagged/packaged docs are build-time snapshots; final receipts live on main and the Release page. GPU tokenizer/window changes remain independent backend configuration.

The previous public [qiwolf.11](docs/releases/v0.34.0-qiwolf.11.md) retains its historical receipt.

The previous public release **[v0.34.0-qiwolf.10](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.10)** fixed disabled-Key deletion. Its historical production upgrade and Key acceptance remain in the [previous receipt](docs/releases/v0.34.0-qiwolf.10.md).

[中文](README.md) | English | [MIT License](LICENSE)

A self-hosted OpenAI-compatible LLM gateway written in Rust, with an Axum server, SQLite storage and a Leptos WebAssembly administration panel. No Node.js runtime or npm build is required.

## Version and Scope

This fork builds on upstream **v0.34.0** and retains that Cargo version. Use the commit identity and current documentation to determine behavior; build the server and panel from the same commit.

The fork provides native Chat, Responses and Messages endpoints, strict text/client-function conversion in all six directions, embeddings, reranking, model listing and standalone DeepSeek Search/Fetch. Users, scoped virtual keys, ancestor quotas, ordered fallback, versioned prices, budgets and durable per-attempt accounting share one executor. Configure source capabilities explicitly; a model name is not evidence of support.

The previous public distribution (historical receipt) is **[v0.34.0-qiwolf.9](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.9)**, source `8598fc070ca4a91e700d5748947d0343766ad06b`: visible Key delivery and clipboard retry, list-only removal of revoked Keys, and authorized reveal/copy of encrypted credentials. Cargo **0.34.0**, schema **40**, Linux x86_64 native and linux/amd64 containers. [Exact-source CI](https://github.com/qiwolf/tokengo/actions/runs/39898866368) and [packaging/release](https://github.com/qiwolf/tokengo/actions/runs/39898890902) passed. All nine assets were anonymously downloaded and verified; native/container Key contracts, anonymous GHCR pull and Compose/container lifecycle checks passed independently. Full build-test tests: 1103 passed, 0 failed, 3 ignored; 22 Python tests passed. Historical focused checks are not added to these totals. See the [release receipt](docs/releases/v0.34.0-qiwolf.9.md). Tagged/packaged docs are pre-publication snapshots; final receipts live on main and in Release notes. Production 237 was not switched to these public binaries by this acceptance.

The previous public distribution (historical receipt) is **[v0.34.0-qiwolf.8](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.8)**, source `ca008c7acfcc53d49c5c59670dd123fc28c37a5b`: shared vector/rerank fallback, independent renewal, virtual-model trash, scoped service access and catalog presentation. [Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39835960738) and [packaging/release](https://github.com/qiwolf/tokengo/actions/runs/39837203892) passed. All nine assets were downloaded anonymously and verified; Linux x86_64 native runtime, anonymous GHCR pull, literal Compose startup and container restart/recreation passed on build-test. See the [release record](docs/releases/v0.34.0-qiwolf.8.md) and [acceptance status](docs/acceptance-status.md). Cargo remains **0.34.0**. Tagged and packaged documentation is a pre-publication snapshot; final receipts are maintained on main and in the GitHub Release notes.


[Generic HTTP pass-through](docs/passthrough.md), introduced in qiwolf.6, uses encrypted static upstream headers and opaque streams. MinerU covers synchronous `/file_parse` and `/health`; its native asynchronous resource chain remains unsupported. The local doc-extract health check and upload both use dedicated gateway authentication. See the [development contract](docs/passthrough-development.md) and [acceptance record](docs/acceptance-status.md) for the separate release, deployment and caller boundaries.

See the [changelog](CHANGELOG.md), [provider channels](docs/provider-channels.md), [accounts and self-service Keys](docs/account-login.md), [protocols and clients](docs/protocols.md), [identity and metering](docs/identity-metering.md), [routing](docs/logical-routing.md), [Search/Fetch](docs/deepseek-search-fetch.md), [independent OpenAI OAuth](docs/openai-oauth.md), [requirements coverage](docs/requirements-status.md), and [deployment](docs/deployment.md). Implementation and local tests do not establish acceptance by every real provider, subscription or CLI.

Conversion rejects semantics it cannot represent. Responses requires full input replay; native signatures and opaque state have ownership restrictions. Native Chat inline audio is an explicit source capability, but separate audio/video/file APIs, native Gemini and arbitrary subscription conversion are not implemented. The official OAuth preview rejects some ordinary API generation fields; review its specific limitations.

Production acceptance is tracked in the [public acceptance status and limits](docs/acceptance-status.md). The Cargo version remains **0.34.0**; fork release tags identify the distributed build.

## Provider Channels and Native Parameters

Channels now presents an explicit provider identity above its connections. Connections retain their original IDs, URLs, credentials, protocols, OAuth bindings, concurrency, pricing and fallback references. Provider grouping is explicit and configurable; there is no fixed provider catalog or count. Assign ungrouped connections explicitly rather than inferring identity from hostnames or model names.

A provider's model switch atomically updates the explicitly listed connections. Disabling preserves aliases and capability configuration. The Models page defaults to public, enabled, routable models; disabled and compatibility-hidden items can be restored through explicit filters. Provider, connection, per-connection model and global logical-model switches have distinct scopes.

Catalog display names are separate from API model identifiers. Supplier and purpose filters organize discovery without changing aliases, upstream mappings, grants, prices or execution order. A catalog purpose may differ from the actual route type; details retain the actual type, and presentation changes do not establish endpoint capabilities. See the [catalog presentation contract](docs/catalog-presentation.md).

Reasoning is a native parameter: `reasoning.effort` for Responses or `reasoning_effort` for Chat, independent of `service_tier`. Explicit client values take precedence. Adding/editing models reads a normal upstream catalog or the bound OAuth account's official catalog without paid inference. Levels, defaults and provenance follow actual metadata. A reasoning boolean does not establish effort levels; unknown remains unknown, with explicit manual confirmation available and labeled as such. Catalog declarations are not inference tests; cross-protocol support depends on the configured protocol.

The standard GPT display names are `gpt-6.1-sol`, `gpt-6-luna` and `gpt-5.6-luna`, subject to the current account's catalog and access. Effort levels are not separate models. Existing effort aliases can remain as explicitly hidden compatibility entries: excluded from `/v1/models` and auto, while direct requests, history and internal references remain intact. No suffix-based renaming or deletion is performed.

For example, send the following to `POST /v1/responses` only if the current account catalog offers this model and effort. A supported `service_tier` is a separate top-level field, not inferred from reasoning effort.

```json
{"model":"gpt-6-luna","stream":true,"store":false,"reasoning":{"effort":"low"},"input":[{"role":"user","content":"Hello"}]}
```

## Accounts, Keys and Observability

New users receive a generated initial password. Reveal/hide changes the eye icon while retaining the button background. After successful creation, the panel attempts to copy the username, display name and password to the system clipboard; failures have a visible retry path.

Pass-through services have a separate CRUD page and an authorized user directory. User and Key `allowed_services` scopes intersect with ancestor restrictions and live route checks; legacy per-Key ACLs retain their original meaning. Ordinary users cannot grant themselves services or read upstream credentials through the directory. See [service authorization](docs/passthrough-user-access.md).

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

The server executable is `target/release/tokengo-server`; panel assets are in `crates/web/dist/`. Keep the executable and panel from the same source revision. See [GitHub Releases](https://github.com/qiwolf/tokengo/releases) for the actual published native and offline-container assets. The Linux packaging CI also retains a same-commit offline image artifact for 14 days. Since `v0.34.0-qiwolf.4`, verified fork release tags also publish version, full-commit and `latest` image tags to `ghcr.io/qiwolf/tokengo`; ordinary PR and manual builds retain review artifacts only. See the [container deployment guide](docs/container-deployment.md) for the multi-stage Dockerfile, non-root Compose setup, persistent data, backup and offline distribution. No production accounts or credentials are included.

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

See the [protocol routing matrix and fallback diagram](docs/protocol-routing-matrix.md) for native selection, bounded bridging and explicit rejection. The first unified-user phase implements atomic user/login creation, browser self-service Keys and point-and-click policies. It adds no schema, user expiry or self-service toggle; browser self-service creation has a fixed 32-active-Key cap. Monetary caps are not raw Token packages. Monthly entitlements and cumulative Token quota remain future work. Administrator-owned [custom probes](docs/custom-probes.md) now support API, OpenAI OAuth Responses, and explicitly enabled standalone Search/Fetch targets with system diagnostic accounting, without charging user budgets. See [requirements coverage](docs/requirements-status.md), the [phased plan](docs/unified-user-and-quota-plan.md) and [acceptance status](docs/acceptance-status.md) for implementation and verification boundaries.


## Provider Key pools and private virtual models

Online provider channels use one API base URL with multiple upstream credentials and models. Explicit local providers support multiple API URLs, including the same IP on different ports, with separate credentials and model mappings per endpoint. Local models are not copied across ports and grouping is never inferred from IP. Ordered or concurrent allocation preserves user affinity across downstream VKeys, with explicit 429/400 switches. Users can create private virtual model names with ordered targets or weekly schedules (UTC or Asia/Shanghai); every attempt rechecks real target permissions, pricing and budget. Native session pins take precedence and committed streams are never replayed. Multi-target embeddings and reranking require server-issued, source-bound compatibility evidence. Saving a strategy probes every authorized source; verified chains can fall back on transport, timeout, server and malformed-output faults, without artificial virtual-model input caps; source-bound evidence expires and renews independently. The 300 × 4096-character workload is a verification sample, not an input limit. See [native capacity and verification](docs/vector-model-capabilities.md). See [vector fallback](docs/vector-fallback.md). See the [behavior contract and flow diagram](docs/provider-key-pools.md) and [acceptance boundaries](docs/acceptance-status.md); source implementation, CI artifacts, Releases and production deployment are verified separately.

Compatibility evidence starts automatic renewal after 18 hours, with a 24-hour hard validity window. Renewal isolates a failed or incompatible source while independently renewing unchanged, verified healthy sources; one unavailable cloud target does not disable a healthy GPU target. Initial verification and manual strategy saves still require the complete chain. All-source failure or invalid global evidence rejects calls with explicit diagnostics rather than silently bypassing checks.

Administrators can grant [shared virtual entry points](docs/shared-virtual-models.md) and mark backing models internal-only. Internal-only access blocks direct calls and private-strategy bypasses; compatibility hiding alone only affects discovery. Shared and private entries support trash/restore, retain history and audit, and remain disabled after restoration. Real target access, pricing, budgets, native session ownership and committed-stream protections still apply.

The previous published [v0.34.0-qiwolf.3](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.3) native and container assets both use runtime commit `f2943d9707e16b231af7e291d4688f3e307ccc4f`; CI, actual image import/lifecycle and target native upgrade were verified. Later documentation-only commits do not replace those binaries.

Upstream Key management lists existing credentials first; leaving the replacement secret blank preserves it. Discover the selected Key's model catalog, search and select multiple models, or enter names manually. Enter the upstream model first and an optional local mapping name second; a blank new mapping uses the upstream name. Existing local names retain their identity and permissions. Catalog discovery sends no inference request.

The published [v0.34.0-qiwolf.4](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.4) adds focused dialogs, separate Key/model editing, explicit credential reveal/copy, cascade deletion of live provider configuration while preserving history, user archival, visible-only new grants with separately retained historical grants, and bounded custom probes. Feature commit `febb2c91ff51516716a4a40c502d7b41316796f9` passed limited live OAuth/Search/Fetch and browser acceptance; release source `15c11c00fa31d3a498a5f86e16d23e20f2bb6503` passed Rust/WASM CI (944 passed, 0 failed, 2 ignored), Linux AMD64 native/container packaging, anonymous GHCR pull and isolated lifecycle checks including recreation with the same volume. Native and offline image downloads plus checksums are attached to the Release; the version image is `ghcr.io/qiwolf/tokengo:v0.34.0-qiwolf.4`.


The previous [v0.34.0-qiwolf.5](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.5) introduced independent operations tokens for trusted deployment agents: one fixed `site_operations` permission, a 90-day default (1–365 days), at most 20 active tokens, one-time secret display, revocation, root security reset and separate audit identity. The token grants broad application management access; it does not provide a host shell or deployment control. The runtime capability endpoint lists 90 registered method/path actions. See [operations token usage](docs/operations-tokens.md). Pass-through route management and manual diagnostics require root and are not added to that action list.

The qiwolf.6 feature release migrated schema 38 to 39; qiwolf.7 retained schema39 and fixed container delivery configuration. The qiwolf.8 feature set uses schema40 for shared virtual models. Operations tokens were introduced in schema38. Back up the database, master key, application, Web assets and service configuration together before upgrading. Rollback requires the matching pre-upgrade database snapshot and old application; do not run an old binary against the upgraded database or silently discard writes made after upgrading.
