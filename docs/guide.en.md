# TokenGo User Guide

[Project home](../README.en.md) · [中文指南](guide.zh-CN.md) · [Release notes](releases/v0.34.0-qiwolf.18.md) · [Publication receipt](acceptance/2026-10-25-qiwolf18-release/README.md)

This guide covers **v0.34.0-qiwolf.18**, Cargo **0.34.0**, database schema **41**. The release is public; use [GitHub Releases](https://github.com/qiwolf/tokengo/releases) and the [receipt](acceptance/2026-10-25-qiwolf18-release/README.md) for exact source and artifact identity. Historical changes remain in the [changelog](../CHANGELOG.md) and [release notes](releases/). Tagged package documentation is a build-time snapshot; later receipts live on main without moving the tag.

TokenGo brings API providers, supported subscription accounts and local models into one self-hosted gateway. The Rust/Axum server uses SQLite, with a Leptos/WebAssembly console. Native and container deployments need no Node.js runtime or separate database service. Current public packages target Linux AMD64; “lightweight” describes this architecture, without a fixed memory, latency or throughput claim.

## Start With a Source

After deployment and administrator initialization, open Providers and Connections → New Channel:

1. **Family and authentication**: choose a logo card, initially displayed in two rows; More Models expands the rest. The family organizes setup, without restricting the real catalog or granting capabilities. Choose API, supported subscription OAuth, or explicit anonymous access for a local service.
2. **Save credentials**: enter the actual API URL and key, or complete your own authorization on the official provider page and save the binding. Online channels use one API URL; local channels may contain multiple independent addresses. An API key cannot substitute for subscription authentication.
3. **Select models**: read the selected credential's catalog, search and select models, then save mappings. Enter the upstream name and an optional public alias; a blank new alias uses the upstream name. Existing mappings retain their identity and grants. Manual entry remains available for verified models. Catalog discovery is not a paid inference test or entitlement confirmation.
4. **Parameters and forwarding**: verify source input/output token bounds, native protocol, permitted client protocols, reasoning controls, defaults and network policy. Unknown capabilities remain unknown rather than being inferred from a brand or model name.
5. **Grant and call**: create users and model/source grants. Configure prices and monetary budgets where required, then let users create their own gateway keys and connect with an authorized public model name.

Closing before the first save creates no empty channel. Closing after credentials are saved retains them; continue through Manage Upstream Keys. If a save response is uncertain, close and refresh to confirm the result before retrying. The system does not delete possibly saved credentials to compensate for a lost response. See [provider setup](provider-channels.md) and [credential pools](provider-key-pools.md).

## API, Subscription and Local Routes

| Route | Setup | Check before use |
| --- | --- | --- |
| API / compatible endpoint | Actual URL, API key and native protocol; explicit parameter profile for a compatible proxy when needed | The real provider, protocol and model metadata, rather than the selected family alone |
| OpenAI / Codex | Existing browser PKCE flow and account-specific catalog | Account access, Responses transport and dedicated controls; see [OpenAI OAuth](openai-oauth.md) |
| Kimi Code | Device authorization and bound-account catalog | Currently only catalog-confirmed native Chat models |
| MiniMax CN / Global | Separate regional device flows and official public model snapshots | `provider_public` is public metadata, not account entitlement |
| GitHub Copilot | Device flow; confirmation page names the registered public application OpenCode | Only enabled models whose account catalog explicitly declares Chat; successful login is not proof of inference |
| Local model service | Multiple addresses with independent key/anonymous credentials, models and bounds | The same IP on different ports does not share protocol or capability declarations; no subscription OAuth in local mode |

Published public-client identifiers for Kimi, MiniMax CN/Global and Copilot ship with the binary/image. New deployments need no independent Client ID registration to start these flows; explicitly configured registrations may override the defaults. Tokens are encrypted server-side. User cookies and local session files are not imported.

Those four new device routes have real endpoint verification for start, pending polling, cancellation and encrypted-state cleanup. **Real-user login, refresh, subscription entitlement, inference and accounting still require per-account acceptance.** Retired Qwen/iFlow flows and Gemini CLI, Claude Code and Antigravity are not advertised as connected subscription routes. See [OAuth providers](oauth-providers.md) for evidence, overrides and limitations.

## Client Protocols and Parameter Matching

The **client protocol** is how an application calls TokenGo: Chat `/v1/chat/completions`, Responses `/v1/responses` or Messages `/v1/messages`. The **provider protocol** is how TokenGo calls the upstream. Prefer a native path; other endpoints require an explicitly enabled conversion that can represent the request. A model family alone does not establish compatibility. See the [protocol routing matrix](protocol-routing-matrix.md).

| Control | Behavior |
| --- | --- |
| Reasoning levels | Chat uses `reasoning_effort`, Responses uses `reasoning.effort`; levels and native thinking switches are matched by provider, authentication, protocol and model. A boolean reasoning flag does not establish low/high levels. |
| `none` | Explicitly disables reasoning, unlike omission, provider defaults or low/minimal. Source-distributed compatibility rules apply only to verified provider/model combinations; manual lists, configured defaults and explicit client controls retain priority. |
| Defaults | Added only when the client omitted the native control. Reasoning levels are not separate models or inferred suffixes. `service_tier` remains independent. |
| JSON Schema | Chat `response_format` and Responses `text.format` convert on representable paths, preserving schema/name/description/strict. Unrepresentable fields are rejected. |
| Output caps | Total-output and answer-only semantics stay distinct. Explicit caps are preserved or rejected, never silently removed or reduced for a retry. Omitted caps may use defaults and learned bounds, recomputed before dispatch and accounting reservation. |
| Budgets | Reasoning budgets, candidate counts and verified source ceilings contribute to reservation. A price table is not capability evidence. OpenAI OAuth has its own cap/alias contract. |

`parameter_profile` can explicitly select a compatible endpoint contract; it does not grant authentication, entitlement or inference acceptance. Built-in recognition and adaptation rules ship in source and artifacts without a private database patch; instance configuration saved in the console remains in that instance's database. Fresh installations still need their own credentials and verified bounds. See [parameter contracts and provider inventory](references/provider-parameter-contracts.md) and [reasoning-off compatibility](reasoning-compatibility.md).

Native signatures, encrypted reasoning and opaque history retain source ownership and cannot be moved freely between accounts or protocols. Responses requires complete input replay. Native Gemini `generateContent`, standalone audio/video/file APIs and arbitrary subscription conversion remain outside scope. See [protocol details](protocols.md).

## Network and Proxies

Expand Network and Proxies in the first setup step. After saving a channel, set a provider override; the parameter step also exposes a specific model-source override. Priority is **model source → provider → family default → system network**. Each layer supports inheritance, explicit direct access or HTTP/HTTPS proxying. Explicit direct access stops environment-proxy inheritance.

Keep the proxy URL separate from credentials. Saved passwords are not returned; changing the address requires an explicit clear/replace decision. SOCKS is not supported. OAuth device, token, refresh, catalog and revocation traffic also uses the selected policies; conflicting effective policies for one account's bound sources are rejected.

Before a new channel exists, OAuth's initial network family follows the authentication provider: GPT for OpenAI, Kimi for Kimi, MiniMax for MiniMax, and Other / Multi-family for Copilot. Selecting a GPT/Claude model family does not alter Copilot's initial authorization exit. See [proxy configuration and verification limits](outbound-proxy.md).

## Routing, Credential Pools and Virtual Models

An online channel's upstream credentials use ordered selection or user-sticky allocation, not duplicate inference fan-out. A user's downstream keys share upstream affinity; native session ownership takes precedence. Explicit 429/400 switches control credential changes. A parameter error does not automatically establish a bad key.

Users can define private virtual models with ordered targets or weekly UTC/Asia-Shanghai schedules. Administrators can grant shared entries and restrict direct calls to backing models. Every attempt still checks actual target permissions, current policy, prices and budgets. Committed streams are not replayed. Virtual names cannot expand access. Disabling retains configuration; deletion removes live configuration while retaining request, price and financial history, and is refused during active/unsettled work. See [key pools and private strategies](provider-key-pools.md), [shared entries](shared-virtual-models.md) and [routing](logical-routing.md).

Multi-target embedding/rerank fallback verifies numeric/output compatibility per source. Evidence is source-bound, expires and renews independently; equal model names are insufficient. Local routes can choose `local_strategy: "round_robin"` to rotate their start while preserving cloud/OAuth affinity. This is not a distributed load-balancing or equal-GPU-utilization guarantee. See [vector fallback](vector-fallback.md), [capacity](vector-model-capabilities.md) and [local node rotation](maintenance/local-vector-balancing.md).

## Accounts, Usage and Accounting

Administrators create logins and grants together. Ordinary users manage their own keys and usage in My Space. User, key and ancestor permissions/budgets intersect; child keys cannot widen grants or bypass total budgets. Disabling a user blocks both login and API calls. Root keeps its separate security identity.

The overview exposes request metrics, active progress and type/source/model/client groups. Usage and Billing separates prices, budgets and per-attempt records. Each retry/fallback reserves and settles against the actual source. Token accounting is separate from request-unit Search/Fetch accounting. Missing usage is not zero, unpriced is not free, and a gateway calculation is not the provider's final invoice. Cumulative monetary budgets are not monthly Token packages or a multi-instance quota service. See [accounts](account-login.md) and [identity/metering](identity-metering.md).

## Diagnostics and Other Services

[Custom probes](custom-probes.md) target supported source types manually or periodically; saving does not dispatch immediately. Diagnostic costs have a separate ledger and do not debit user budgets. A passing probe is not blanket business-path acceptance.

[Standalone Search/Fetch](deepseek-search-fetch.md) does not automatically replace a CLI's hosted tools. [HTTP pass-through](passthrough.md) adds authorized native services, preserving opaque bodies within its contract rather than claiming every asynchronous resource chain. Users see only granted services and receive no upstream credentials; see [service access](passthrough-user-access.md).

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

The server executable is `target/release/tokengo-server`; panel assets are in `crates/web/dist/`. Keep the executable and panel from the same source revision. See [GitHub Releases](https://github.com/qiwolf/tokengo/releases) for the actual published native and offline-container assets. The Linux packaging CI also retains a same-commit offline image artifact for 14 days. Since `v0.34.0-qiwolf.4`, verified fork release tags also publish version, full-commit and `latest` image tags to `ghcr.io/qiwolf/tokengo`; ordinary PR and manual builds retain review artifacts only. See the [container deployment guide](container-deployment.md) for the multi-stage Dockerfile, non-root Compose setup, persistent data, backup and offline distribution. No production accounts or credentials are included.

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

1. Complete setup and save the once-displayed gateway key before exposing the service.
2. Follow [Start With a Source](#start-with-a-source) for family, authentication, models and parameter review. Add backup credentials through the existing channel's Manage Upstream Keys action.
3. Ensure provider, source and model are enabled; configure users, real source prices and optional monetary budgets.
4. Users create their own keys. OpenAI-compatible clients normally use `http://127.0.0.1:3160/v1`, a gateway key and an allowed public model name. Messages clients should follow their base-URL convention for `/v1/messages` without duplicating `/v1`.

## Environment Variables

| Variable | Default or behavior | Purpose |
| --- | --- | --- |
| TOKENGO_LISTEN | 127.0.0.1:3160 | HTTP bind address |
| TOKENGO_DB | tokengo.db | SQLite database path |
| TOKENGO_MASTER_KEY_FILE | master.key | Persistent encryption key file |
| TOKENGO_WEB_DIR | Auto-discovery | Built panel directory; set explicitly in production |
| TOKENGO_LOG_DIR | ./logs in server entry point | Runtime file log directory |
| TOKENGO_OPENAI_PROXY | Default HTTP client behavior when unset | Legacy OpenAI inheritance baseline; explicit panel direct/proxy policies take priority, with other sources governed by their network policies |

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

Tests cover local HTTP/SSE fixtures, permissions, accounting, recovery, protocols and frontend logic. Passing fixtures or isolated browser acceptance does not certify all real providers or a deployment. Check the exact commit's CI and [public acceptance status](acceptance-status.md) instead of treating a historical test count as current proof. Unset proxy environment variables if a proxy intercepts loopback fixtures.

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

MIT; see [LICENSE](../LICENSE) and [NOTICE](../NOTICE). FreeLLMAPI informed compatibility behavior. Only connect upstream services you are authorized to use and respect their terms and quotas.
