# TokenGo

**轻量部署，简单接入，统一管理大模型。**

**A lightweight gateway. A simpler way to connect and manage your models.**

TokenGo 是面向个人与小团队的自托管大模型网关：把 API 服务、可接入的订阅账号和本地模型整理到同一个控制台，按来源能力向客户端提供 Chat、Responses、Messages 等接口，并统一管理模型、密钥、权限、路由和账务。

TokenGo is a self-hosted large language model (LLM) gateway for individuals and small teams. Bring API providers, supported subscription accounts and local models into one console. Expose Chat, Responses, Messages and other endpoints according to source capabilities, with shared model, key, access, routing and accounting management.

[中文使用指南](docs/guide.zh-CN.md) · [English guide](docs/guide.en.md) · [English first](README.en.md) · [Releases](https://github.com/qiwolf/tokengo/releases) · [MIT](LICENSE)

当前正式版：**[v0.34.0-qiwolf.20](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.20)** · [版本说明](docs/releases/v0.34.0-qiwolf.20.md) · [发布记录](docs/acceptance/2026-10-26-qiwolf20-release/README.md)。

Current stable release: **[v0.34.0-qiwolf.20](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.20)** · [Release notes](docs/releases/v0.34.0-qiwolf.20.md) · [Publication record](docs/acceptance/2026-10-26-qiwolf20-release/README.md).

## 84 秒认识 TokenGo · Meet TokenGo in 84 seconds

https://github.com/user-attachments/assets/6a26c4d3-f4c5-4523-bc90-8cffe3a7c73f

**[下载 1080P 视频 · Download the 1080p video](docs/media/tokengo-intro.mp4)**

1920 × 1080 · 84 秒 / 84 seconds · 中英双语字幕 / Chinese & English captions · [无背景音乐版 / SFX only](docs/media/tokengo-intro-no-bgm.mp4)

## 为什么选择 TokenGo · Why TokenGo

| 特点 | 中文 | English |
| --- | --- | --- |
| 轻量部署 · Lightweight | Rust 服务端、SQLite 与 WebAssembly 控制台；原生包或容器部署，无需 Node.js 运行时或额外数据库服务。 | A Rust server, SQLite and a WebAssembly console, distributed as native packages and containers. No Node.js runtime or separate database service. |
| 按流程接入 · Guided setup | 从家族 Logo 选择开始，分清 API 与订阅 OAuth，再保存凭据、选模型、检查参数和协议。 | Choose a model family, select API or supported subscription OAuth, save credentials, select models and review parameters and protocols. |
| 按来源适配 · Source-aware controls | 区分推理档位、thinking 开关、JSON Schema 和输出上限；显式参数优先，不能等价表达时明确报错。 | Match reasoning levels, thinking switches, JSON Schema and output caps to the source. Explicit controls take priority; unrepresentable requests are rejected. |
| 出口可控 · Network control | 模型接入、渠道、家族逐层设置 HTTP/HTTPS 代理，可继承或明确直连。 | Set HTTP(S) proxy policies per model source, provider or family, with inheritance and explicit direct access. |
| 统一路由 · Unified routing | 上游 Key 池、私有/共享虚拟模型、有序回退与时段策略；本地向量和重排可配置等权轮询。 | Upstream credential pools, private/shared virtual models, ordered fallback and schedules; optional round-robin for local embedding/rerank sources. |
| 权限与账务 · Access and accounting | 用户自助 Key、逐层授权与预算、请求进度及逐次尝试账本；未知用量或费用不冒充零。 | Self-service keys, intersecting permissions and budgets, request progress and per-attempt ledgers. Unknown usage and cost remain explicit. |

## 从部署到调用 · From deployment to your first request

1. **部署 / Deploy** — 从 [Releases](https://github.com/qiwolf/tokengo/releases) 获取原生包或容器文件，按[原生部署](docs/deployment.md)或[容器部署](docs/container-deployment.md)启动并完成管理员初始化。Install a matching native package or container and initialize the administrator.
2. **接入来源 / Add a source** — 在“供应商与接入”点“新建渠道”，选择家族和认证线路；API 填真实地址与 Key，OAuth 在官方页面完成账号授权。In Providers and Connections, use New Channel to choose a family and authentication route; enter API credentials or complete the supported provider's authorization.
3. **选择并核对 / Select and review** — 读取目录、保存模型映射，核对输入/输出上界、原生协议、推理档位及网络出口；目录可读不等于推理已通过。Save model mappings, verify source bounds, native protocols, reasoning controls and network policy. Catalog discovery is not an inference test.
4. **授权并调用 / Grant and call** — 创建用户、授予模型和来源权限，按需配置价格与预算；用户创建自己的 Key，客户端使用网关地址、Key 和获准模型名。Create users and grants, configure prices and budgets where needed, then connect with a user's gateway key and authorized model name.

完整操作见[中文指南](docs/guide.zh-CN.md) / [English guide](docs/guide.en.md)。

OpenAI 兼容客户端通常填写 `http://127.0.0.1:3160/v1`；远程部署请替换为自己的 HTTPS 地址。

For OpenAI-compatible clients, the local base URL is `http://127.0.0.1:3160/v1`. Use your own HTTPS address for a remote deployment.

```sh
# Replace both placeholders with your configured gateway key and model name.
export TOKENGO_API_KEY='YOUR_GATEWAY_KEY'
curl http://127.0.0.1:3160/v1/chat/completions \
  -H "Authorization: Bearer $TOKENGO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_MODEL_NAME","messages":[{"role":"user","content":"Hello"}]}'
```

## 能力与边界 · Capabilities and scope

支持 Chat、Responses、Messages、向量、重排、模型清单、独立 Search/Fetch 及按授权的 HTTP 透传。**客户端接入协议**是应用调用 TokenGo 的协议；**供应方接口协议**是 TokenGo 调用上游的协议。两者可以相同，也可以经过允许的可表达转换，不能只凭模型家族判断兼容。

Supports Chat, Responses, Messages, embeddings, reranking, model listing, standalone Search/Fetch and authorized HTTP pass-through. The **client protocol** is how an application calls TokenGo; the **provider protocol** is how TokenGo calls the upstream. They may be native or use an explicitly permitted, representable conversion.

订阅线路包括既有 OpenAI/Codex 浏览器授权，以及 Kimi Code、MiniMax 中国/Global、GitHub Copilot 设备授权。后四条已验证真实设备端点的发起、轮询 pending 和取消；真实用户授权、刷新、权益、推理及账务仍需逐账号验收。Copilot 授权页显示公开注册应用 OpenCode，目前仅开放账号目录明确支持 Chat 的模型；MiniMax 公共目录不代表订阅权益。

Subscription routes include the existing OpenAI/Codex browser flow and device authorization for Kimi Code, MiniMax CN/Global and GitHub Copilot. The latter four flows have verified start, pending polling and cancellation against real endpoints; real-user login, refresh, entitlement, inference and accounting still require account-specific acceptance. Copilot displays the registered public application OpenCode and currently accepts only catalog-declared Chat models. MiniMax's public catalog does not establish subscription entitlement.

`none` 是显式关闭推理，与省略参数不同；适配按真实来源、认证、协议和型号匹配，不给未知兼容服务自动套用能力。显式输出上限保留或明确拒绝，不静默删除或缩小；JSON Schema 只在可保真路径转换。完整规则见[参数合同](docs/references/provider-parameter-contracts.md)。

`none` explicitly disables reasoning and differs from omission. Matching uses the actual provider, authentication route, protocol and model. Explicit output caps are preserved or rejected rather than silently removed or reduced; JSON Schema conversion is limited to representable paths. See the [parameter contract](docs/references/provider-parameter-contracts.md).

原生签名和不透明状态受来源归属约束；流式输出提交后不重放。未实现 Gemini 原生 `generateContent`、独立音视频/文件 API 或所有品牌订阅接入；代理目前支持 HTTP/HTTPS，未支持 SOCKS。公开产物当前为 Linux AMD64。“轻量”描述部署结构，不承诺固定资源消耗或吞吐数字。

Native signatures and opaque state retain source ownership; committed streams are not replayed. Native Gemini `generateContent`, separate audio/video/file APIs, every subscription brand and SOCKS are outside current scope. Public packages target Linux AMD64. “Lightweight” describes deployment structure, without fixed resource or throughput claims.

## 文档 · Documentation

| 主题 / Topic | 入口 / Read more |
| --- | --- |
| 完整使用、配置与示例 / Full usage, configuration and examples | [中文指南](docs/guide.zh-CN.md) · [English guide](docs/guide.en.md) |
| 部署、升级与备份 / Deployment, upgrades and backups | [Deployment](docs/deployment.md) · [Containers](docs/container-deployment.md) |
| 渠道、凭据与 OAuth / Channels, credentials and OAuth | [接入流程 / Provider setup](docs/provider-channels.md) · [Key pools](docs/provider-key-pools.md) · [OAuth](docs/oauth-providers.md) |
| 参数、推理关闭与网络 / Parameters, reasoning off and networking | [参数合同 / Parameter matching](docs/references/provider-parameter-contracts.md) · [Reasoning off](docs/reasoning-compatibility.md) · [代理 / Proxies](docs/outbound-proxy.md) |
| 用户、自助密钥与权限 / Users, self-service keys and access | [Accounts](docs/account-login.md) · [Identity and metering](docs/identity-metering.md) |
| 协议、客户端与路由 / Protocols, clients and routing | [Protocols](docs/protocols.md) · [Protocol matrix](docs/protocol-routing-matrix.md) · [Routing](docs/logical-routing.md) · [Shared models](docs/shared-virtual-models.md) |
| 向量、诊断与透传 / Vectors, diagnostics and pass-through | [Vector fallback](docs/vector-fallback.md) · [Local rotation](docs/maintenance/local-vector-balancing.md) · [Probes](docs/custom-probes.md) · [Pass-through](docs/passthrough.md) |
| 版本、验收与已知限制 / Versions, acceptance and limits | [Changelog](CHANGELOG.md) · [Acceptance status](docs/acceptance-status.md) · [Release notes](docs/releases/) |

发行版本以 [GitHub Releases](https://github.com/qiwolf/tokengo/releases) 的实际附件与对应验收记录为准；历史版本说明与技术细节保留在完整指南中。

Published versions are identified by the actual Release assets and their acceptance records. Historical release details and technical guidance remain in the full guides.

TokenGo 基于 [shang1626/tokengo](https://github.com/shang1626/tokengo)，采用 [MIT 许可证](LICENSE)，归属见 [NOTICE](NOTICE)。

Based on [shang1626/tokengo](https://github.com/shang1626/tokengo), distributed under the [MIT License](LICENSE); see [NOTICE](NOTICE) for attribution.
