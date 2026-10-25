# 账号授权来源与实施边界

状态：2026-10-25，本地源码实现；未编译、未运行测试、未创建真实设备授权、未交换真实 token、未执行上游推理或账务验收。不能把本文件当作生产可用性验收。

## 认证与推理分别判断

`GET /api/oauth/providers` 分别返回 `authentication_available` 和 `inference_available`。公开登录页可达、OAuth 协议已实现、部署已有独立客户端注册、账号可调用模型是不同事实。`available` 只表示当前可启动认证，不授予模型调用能力。

| 来源 | 当前认证实现 | 推理与目录 | 部署需要的独立注册 |
|---|---|---|---|
| OpenAI / Codex | 既有独立注册 PKCE，新增取消及迟到完成保护 | 既有 Responses 与专属目录 | 沿用独立注册流程 |
| Kimi Code | 设备授权、轮询、刷新、密文存储、本地撤销 | 固定 `https://api.kimi.com/coding/v1`；获取该账号目录后，只允许已确认的原生 Chat 型号 | `TOKENGO_KIMI_OAUTH_CLIENT_ID` |
| MiniMax 中国 | 设备授权与 PKCE、刷新、远程及本地撤销 | 固定 `https://agent.minimax.cn/mavis/api/v1/llm/v1`，原生 Messages；只支持手工填写已核实型号和边界，不假造账号目录 | `TOKENGO_MINIMAX_CN_OAUTH_CLIENT_ID` |
| MiniMax Global | 与中国区独立的设备授权及凭据 | 固定 `https://agent.minimax.io/mavis/api/v1/llm/v1`，原生 Messages，手工型号 | `TOKENGO_MINIMAX_GLOBAL_OAUTH_CLIENT_ID` |
| GitHub / Copilot | 自有 GitHub OAuth App 的 device flow，官方用户端点确认账户 | GitHub 授权不自动获得 Copilot 模型推理权限；未确证独立服务令牌合同前明确不允许推理绑定 | `TOKENGO_GITHUB_OAUTH_CLIENT_ID`，注册应用需开启 device flow |

客户端 ID 为对应部署申请的公开注册标识，不是 API Key，也不是秘密。不能用官方 CLI 的注册标识冒充原生应用；原生 CLI 的非公开 secret、用户本地会话文件和 Cookie 不被导入。本实现拒绝已知原生客户端 ID。未配置时，接口返回 `oauth_client_registration_required`，不会显示假授权成功。

Claude Code 的官方客户端认证使用范围不等于第三方网关订阅授权。Qwen 的旧 OAuth 已停用。Antigravity 需要独立注册与对应服务传输合同；iFlow 的公开资料尚不足以实施独立 OAuth 协议。这些来源保留原因与官方说明，不伪造可绑定认证。

## 设备流程

根管理员启动 `POST /api/oauth/device/start`，提交 `provider`、`label` 和可选的当前渠道父级 `provider_id`。服务端向固定官方端点申请设备授权，返回短期 attempt ID、用户码和通过域名白名单校验的验证链接。设备码与 PKCE verifier 只保存在密文中，管理 API 不返回 bearer/refresh token。

前端按 `interval` 秒轮询 `POST /api/oauth/device/{id}/poll`。数据库记录下一次轮询时间与网络请求 lease，处理 pending、slow_down、过期和拒绝；同一结果最多完成一次。完成时账号插入与 pending 密文清理处于同一事务。关闭、改变家族/来源或取消时调用 `POST /api/oauth/device/{id}/cancel`；即使 token 请求在途中，取消后的结果也不能创建账号。

MiniMax 的官方兼容形状有标准 `device_code`，也有 account `user_code` 轮询。`expired_in` 小于 `1e12` 时按相对秒处理，否则按绝对 Unix 毫秒减当前时间并向上取秒；account 形状的 `interval` 则是毫秒。它们不能混用单位。轮询周期不被自动缩短。

## 刷新、撤销与模型校验

刷新采用数据库 lease 与凭据版本 CAS，并脱离前端等待请求运行，避免请求取消丢失已轮换 refresh token。凭据只从所属账号读取，不回退到渠道 API Key。刷新结果不得覆盖已撤销、已变更或 lease 不再属于本请求的账号。

Kimi 目录缓存按 provider、client ID、账号凭据版本和有效期绑定。token 轮换使缓存失效后，发送前自动重新取得目录。官方 SDK 对缺省或 `kimi` 的目录协议使用 Chat，对 `anthropic` 和 `response` 分别使用 Messages/Responses；当前实现只开放前者，其他协议或未来未知声明明确拒绝，不硬发 Chat。

撤销先清理本地密文并增加凭据版本，再尝试已明确的官方撤销端点。`remote_revocation_confirmed=false` 不表示本地仍可用。没有已证实远程撤销合同的来源不调用猜测端点。MiniMax 的 bearer 显式放在 `Authorization`，不误写成 `x-api-key`。

## 网络出口

认证开始可带所属渠道父级。加密 pending 与 token bundle 保留其 origin，用于未绑定账号的后续交换与刷新。全部设备请求、token、JWKS、discovery、目录、用户资料与撤销通过统一的来源/渠道/家族出口选择。已有账号的多来源代理不一致时明确报错，不能悄悄回退直连。

## 固定官方依据

- [Kimi CLI OAuth](https://github.com/MoonshotAI/kimi-cli/blob/9ab1286b8fe4e6bcd116949a27ce5e0ac3389c82/src/kimi_cli/auth/oauth.py)、[平台与目录](https://github.com/MoonshotAI/kimi-cli/blob/9ab1286b8fe4e6bcd116949a27ce5e0ac3389c82/src/kimi_cli/auth/platforms.py)。新 Kimi Code SDK 的协议与真实产品身份规则另按 `MoonshotAI/kimi-code` 固定 `7ccac2b45b8a3481c76f771bb99c043b562de2c5` 核对。
- [MiniMax OAuth contracts](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/oauth-core/src/contracts.ts)、[设备与刷新协议](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/oauth-core/src/oauth-client.ts)、[固定端点](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/oauth-core/src/endpoint-config.ts)、[managed 推理配置](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/config/src/config.ts)。
- [Copilot SDK 自有 GitHub OAuth 应用指南](https://github.com/github/copilot-sdk/blob/c57638f4a7ce2255a34192d5035932d0e109bbdd/docs/setup/github-oauth.md)。

后续需在用户允许构建后完成编译和 mock/集成回归，再用真实独立注册按来源验证登录、刷新、取消、模型协议、输出截断、用量和账务；每条线路分别验收。
