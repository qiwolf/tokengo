# TokenGo

**A lightweight gateway. A simpler way to connect and manage your models.**

**轻量部署，简单接入，统一管理大模型。**

TokenGo is a self-hosted large language model (LLM) gateway for individuals and small teams. Connect providers through one OpenAI-compatible application programming interface (API) and manage models, keys, users, routing and usage in one console.

TokenGo 是面向个人与小团队的自托管大模型网关：将多个服务商接入一个兼容 OpenAI 的应用程序接口（API），在同一个控制台管理模型、密钥、用户、路由和用量。

[English guide](docs/guide.en.md) · [中文使用指南](docs/guide.zh-CN.md) · [中文优先](README.md) · [Releases](https://github.com/qiwolf/tokengo/releases) · [MIT](LICENSE)

Current stable release: **[v0.34.0-qiwolf.15](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.15)** · [Release notes](docs/releases/v0.34.0-qiwolf.15.md) · [Publication record](docs/acceptance/2026-10-25-qiwolf15-release/README.md).

当前正式版：**[v0.34.0-qiwolf.15](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.15)** · [版本说明](docs/releases/v0.34.0-qiwolf.15.md) · [发布记录](docs/acceptance/2026-10-25-qiwolf15-release/README.md)。

下一版候选 / Next release candidate: **qiwolf.16** — [本地节点轮询、OAuth与日志修复 / local node rotation, OAuth and logging fixes](docs/releases/v0.34.0-qiwolf.16.md)。

## Meet TokenGo in 60 seconds · 60 秒认识 TokenGo

https://github.com/user-attachments/assets/82866974-b13c-4f5c-8606-c03348bd94d1

**[下载 1080P 视频 · Download the 1080p video](docs/media/tokengo-intro.mp4)**

1920 × 1080 · 约 60 秒 / about 60 seconds · 中英双语字幕 / Chinese & English captions

## Why TokenGo · 为什么选择 TokenGo

| 特点 | 中文 | English |
| --- | --- | --- |
| 轻量 · Lightweight | Rust 服务端、SQLite 存储、WebAssembly 管理面板；部署无需 Node.js 运行时，也无需额外数据库服务。 | A Rust server, SQLite storage and a WebAssembly console. No Node.js runtime or separate database service to deploy. |
| 简单 · Simple | 在控制台配置服务商与模型，给客户端填写统一地址、密钥和模型名。 | Configure providers and models in the console; give your client a gateway URL, key and model name. |
| 统一 · Unified | 同一入口管理多服务商、模型别名、上游密钥池，以及私有和共享虚拟模型。 | Manage providers, model aliases, upstream key pools, and private or shared virtual models together. |
| 可控 · Controlled | 按用户与密钥分配权限、限额和预算；普通用户自助管理自己的密钥。 | Set permissions, quotas and budgets per user and key. Users manage their own keys. |
| 可追踪 · Observable | 查看请求进度、用量与逐次尝试账本；未知用量与未知费用单独标记。 | Inspect request progress, usage and per-attempt ledgers. Unknown usage and costs stay explicitly unknown. |

## From deployment to your first request · 从部署到调用

1. **部署 / Deploy** — 从 [Releases](https://github.com/qiwolf/tokengo/releases) 获取原生程序或容器部署文件，按[部署指南](docs/container-deployment.md)启动。Download a native package or container deployment files and follow the [deployment guide](docs/deployment.md).
2. **配置 / Configure** — 首次打开控制台设置管理员密码，添加服务商凭据和模型，再创建用户并分配权限。Set your administrator password, add provider credentials and models, then create users and assign access.
3. **接入 / Connect** — 客户端填写网关地址、自己的密钥及已授权模型名。Enter the gateway URL, your own key and an authorized model name in your client.

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

## Capabilities and scope · 能力与边界

支持 Chat、Responses、Messages、向量、重排、模型清单及独立 Search/Fetch；还可按授权接入通用 HTTP 透传服务。有序回退遵守来源权限、预算与会话归属，流式输出提交后不重放。

Supports Chat, Responses, Messages, embeddings, reranking, model listing and standalone Search/Fetch, plus authorized HTTP pass-through services. Ordered fallback respects source permissions, budgets and session ownership; committed streams are never replayed.

协议转换仅覆盖可保真的文本与客户端函数工具；签名、不透明状态和官方 OAuth 预览各有约束。未实现 Gemini 原生协议或独立音视频/文件 API。实际能力取决于来源配置，测试通过不代表所有供应商与客户端都已验收。当前公开交付架构为 Linux AMD64；“轻量”描述部署结构，不承诺固定内存、延迟或吞吐数字。

Protocol conversion covers representable text and client function tools. Signatures, opaque state and the official OAuth preview have specific constraints. Native Gemini and separate audio/video/file APIs are not implemented. Capabilities depend on source configuration; passing tests does not establish acceptance for every provider or client. Public distributions currently target Linux AMD64. “Lightweight” describes the deployment architecture, without a fixed memory, latency or throughput claim.

## Documentation · 文档

| 主题 / Topic | 入口 / Read more |
| --- | --- |
| 完整使用、配置与示例 / Full usage, configuration and examples | [中文指南](docs/guide.zh-CN.md) · [English guide](docs/guide.en.md) |
| 部署、升级与备份 / Deployment, upgrades and backups | [Deployment](docs/deployment.md) · [Containers](docs/container-deployment.md) |
| 渠道与上游密钥池 / Providers and upstream key pools | [Provider channels](docs/provider-channels.md) · [Key pools](docs/provider-key-pools.md) |
| 用户、自助密钥与权限 / Users, self-service keys and access | [Accounts](docs/account-login.md) · [Identity and metering](docs/identity-metering.md) |
| 协议、客户端与路由 / Protocols, clients and routing | [Protocols](docs/protocols.md) · [Routing](docs/logical-routing.md) |
| 版本、验收与已知限制 / Versions, acceptance and limits | [Changelog](CHANGELOG.md) · [Acceptance status](docs/acceptance-status.md) · [Release notes](docs/releases/) |

发行版本以 [GitHub Releases](https://github.com/qiwolf/tokengo/releases) 的实际附件与对应验收记录为准；历史版本说明与技术细节保留在完整指南中。

Published versions are identified by the actual Release assets and their acceptance records. Historical release details and technical guidance remain in the full guides.

TokenGo 基于 [shang1626/tokengo](https://github.com/shang1626/tokengo)，采用 [MIT 许可证](LICENSE)，归属见 [NOTICE](NOTICE)。

Based on [shang1626/tokengo](https://github.com/shang1626/tokengo), distributed under the [MIT License](LICENSE); see [NOTICE](NOTICE) for attribution.
