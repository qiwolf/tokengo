# 账号授权来源与实施边界

状态：2026-10-25，本轮已在 build-test 对 Kimi、MiniMax 中国、MiniMax Global、GitHub Copilot 的真实官方设备授权端点发起请求，四项均 HTTP 200，并返回设备授权所需字段。未登录真实用户、未取得真实用户 token、未执行真实模型推理；授权完成、刷新与真实推理及账务仍须用户账号验收。精确发行源码、完整测试和公开包结果见[发行收据](acceptance/2026-10-25-qiwolf18-release/README.md)，不能用这些结果代替账号验收。

## 在控制台完成接入

1. 打开“供应商与接入 → 新建渠道”，选择模型家族，再选择“订阅OAuth”和实际认证来源。家族不是账号身份；例如Copilot可提供多个家族，不能把所选家族当作OAuth供应方。
2. 在“网络与代理”确认实际认证出口。首次尚未保存渠道时采用认证来源默认家族；授权服务、令牌交换和后续目录均需可达，不只验证浏览器能打开登录页。
3. Kimi、MiniMax中国/Global、Copilot点击开始设备授权，打开界面给出的官方验证链接，按提示确认用户码和账号。页面按官方间隔检查状态；`pending`表示等待用户确认，`slow_down`会延长等待，不需重复点击新建授权。Copilot页面显示的应用是OpenCode。
4. OpenAI使用浏览器PKCE流程：按界面打开授权页，最终回到本次指定的本地回调地址，将完整回调URL粘回面板完成校验。回调只用于当前尝试，不要粘贴访问令牌或另一次授权结果。CLI辅助流程另见[OpenAI OAuth](openai-oauth.md)。
5. 授权完成后选中账号，确认来源输入/输出边界并保存凭据绑定，再进入“选择模型”。Kimi/Copilot只接受账号目录及协议核验的型号；MiniMax公共目录是选择参考，不是订阅权益保证。最后在“参数与转发”检查原生协议、转换、推理档位和具体接入代理。

设备授权中的取消、关闭或改变家族/认证来源会使当前尝试失效；已经保存的账号/凭据与短期授权尝试不是同一对象。首次凭据保存后关闭新建流程会保留渠道，可回到“管理上游Key”继续，不要重复创建。设备码、完整授权链接、回调URL和token均按敏感材料处理，不放入截图、工单或普通日志。

## 常见状态如何处理

| 现象 | 判断与操作 |
| --- | --- |
| 提示客户端标识无效 | 检查管理员设置的`TOKENGO_*_OAUTH_CLIENT_ID`覆盖值；有默认公开标识的四条设备线路无需为新安装先注册独立Client ID |
| 等待确认或要求降频 | 在官方页面完成当前账号确认，并等待面板按间隔继续；不是已经授权成功 |
| 授权过期、被拒绝或取消 | 重新开始一次授权；旧结果不能迟到绑定新账号 |
| 已登录但目录或推理失败 | 分别检查账号权益、地区/线路、目录支持的原生协议、输入输出边界及实际网络出口，不改用另一账号的目录代替 |
| 首次页面可打开，服务端交换失败 | 浏览器网络与TokenGo服务器出口不同；按[代理层级](outbound-proxy.md)检查认证来源实际策略 |
| 来源不可开始授权 | 阅读界面原因；停用的旧OAuth或仅限官方客户端的流程不能靠填API Key、公开Client ID或Cookie绕过 |

## 认证与推理分别判断

`GET /api/oauth/providers` 分别返回 `authentication_available` 和 `inference_available`。它们表示当前代码支持启动认证和对应传输，不代表某个账号已有订阅权益，也不是用户授权成功证明。`available` 只表示可启动认证。

| 来源 | 当前认证实现 | 推理与目录 | 默认公开客户端标识 |
|---|---|---|---|
| OpenAI / Codex | 沿用既有浏览器 PKCE 授权、取消与迟到完成保护 | 既有 Responses 与专属目录 | 沿用现有配置，不称作 TokenGo 独立注册 |
| Kimi Code | 设备授权、轮询、刷新、密文存储、本地撤销 | 固定 `https://api.kimi.com/coding/v1`；从账号目录中只允许原生 Chat 型号 | Kimi 官方源码公开 native client ID |
| MiniMax 中国 | 设备授权与 PKCE、刷新、远程及本地撤销 | 固定 `https://agent.minimax.cn/mavis/api/v1/llm/v1` 原生 Messages；读取官方公共模型快照 | 官方 `mcode-public` |
| MiniMax Global | 与中国区独立的设备授权及凭据 | 固定 `https://agent.minimax.io/mavis/api/v1/llm/v1` 原生 Messages；读取所在地区的公共快照 | 官方 `mcode-public` |
| GitHub / Copilot | 参照 GitHub 正式支持的 OpenCode device flow，用户端点确认身份 | GitHub OAuth bearer 发往固定 `https://api.githubcopilot.com`；只允许目录明确声明 `/chat/completions` 且未禁用的模型 | OpenCode 公开 native client ID；授权页面显示 OpenCode |

客户端 ID 是公开应用编号，不是 API Key 或秘密。当前有出处、已验证设备授权发起成功的公开 native client ID 默认内置，不再要求部署者申请独立注册，也不再用黑名单拒绝这些 ID。若部署有自己的客户端注册，可用 `TOKENGO_KIMI_OAUTH_CLIENT_ID`、`TOKENGO_MINIMAX_CN_OAUTH_CLIENT_ID`、`TOKENGO_MINIMAX_GLOBAL_OAUTH_CLIENT_ID`、`TOKENGO_GITHUB_OAUTH_CLIENT_ID` 覆盖；显式错误配置仍会拒绝。不会导入用户本机 Cookie、非公开客户端 secret 或现有会话文件。

GitHub 的无到期 OAuth token 按无到期凭据保存，不人为假设一天后过期；有明确 `expires_in` 时仍执行边界校验，到期且没有 refresh token 时只将该账号标记为需重新授权。Copilot 的订阅权益与模型可用性以真实账号目录/请求结果为准，不能因 Github 登录成功就声称已验证可推理。当前只开放目录明确支持 Chat 的型号；Responses-only、Messages-only 和未知端点不被强行转为 Chat。

MiniMax 官方公共快照位于各地区 `https://agent.minimax.{cn,io}/mavis/api/v1/models`，本轮从 build-test 匿名读取均 HTTP 200。结果标记 `catalog_scope=provider_public`，只表示官方发布型号与参数，不冒充账号权益目录。请求公共快照不携带用户 bearer；未知 provider package 不猜测其原生协议。

## 未开放来源的已知原因

- Claude Code 官方订阅凭据使用范围不等于第三方网关授权；保留官方说明。
- Qwen 官方文档明确旧 OAuth 已于 2026-04-15 停用，不重建废弃接入。
- iFlow 官方仓库公告 CLI 于 2026-04-17 关闭，已不是简单“缺少注册配置”。
- Gemini CLI 官方 FAQ 明确第三方不得复用 CLI OAuth 后端，应使用 Google AI Studio / Vertex API 接入；不以公开客户端字段可读为由误报支持。
- Antigravity 未确认可供本网关使用的完整授权与传输合同，保持不可用并说明原因。

## 设备流程

根管理员启动 `POST /api/oauth/device/start`，提交 `provider`、`label` 和可选的当前渠道父级 `provider_id`。服务端向固定官方端点申请设备授权，返回短期 attempt ID、用户码和通过域名白名单校验的验证链接。设备码与 PKCE verifier 只保存在密文中，管理 API 不返回 bearer/refresh token。

前端按 `interval` 秒轮询 `POST /api/oauth/device/{id}/poll`。数据库记录下一次轮询时间与网络请求 lease，处理 pending、slow_down、过期和拒绝；同一结果最多完成一次。完成时账号插入与 pending 密文清理处于同一事务。关闭、改变家族/来源或取消时调用 `POST /api/oauth/device/{id}/cancel`；即使 token 请求在途中，取消后的结果也不能创建账号。

MiniMax 的官方兼容形状有标准 `device_code`，也有 account `user_code` 轮询。`expired_in` 小于 `1e12` 时按相对秒处理，否则按绝对 Unix 毫秒减当前时间并向上取秒；account 形状的 `interval` 则是毫秒。它们不能混用单位。轮询周期不被自动缩短。

## 刷新、撤销与模型校验

刷新采用数据库 lease 与凭据版本 CAS，并脱离前端等待请求运行，避免请求取消丢失已轮换 refresh token。凭据只从所属账号读取，不回退到渠道 API Key。刷新结果不得覆盖已撤销、已变更或 lease 不再属于本请求的账号。

Kimi/Copilot 目录缓存按 provider、client ID、账号凭据版本和有效期绑定。token 轮换使缓存失效后，发送前自动重新取得目录。官方 SDK 对缺省或 `kimi` 的目录协议使用 Chat，对 `anthropic` 和 `response` 分别使用 Messages/Responses；当前实现只开放前者，其他协议或未来未知声明明确拒绝，不硬发 Chat。

撤销先清理本地密文并增加凭据版本，再尝试已明确的官方撤销端点。`remote_revocation_confirmed=false` 不表示本地仍可用。没有已证实远程撤销合同的来源不调用猜测端点。MiniMax 的 bearer 显式放在 `Authorization`，不误写成 `x-api-key`。

## 网络出口

认证开始可带所属渠道父级。加密 pending 与 token bundle 保留其 origin，用于未绑定账号的后续交换与刷新。全部设备请求、token、JWKS、discovery、目录、用户资料与撤销通过统一的来源/渠道/家族出口选择。已有账号的多来源代理不一致时明确报错，不能悄悄回退直连。

## 固定官方依据

- [Kimi CLI OAuth](https://github.com/MoonshotAI/kimi-cli/blob/9ab1286b8fe4e6bcd116949a27ce5e0ac3389c82/src/kimi_cli/auth/oauth.py)、[平台与目录](https://github.com/MoonshotAI/kimi-cli/blob/9ab1286b8fe4e6bcd116949a27ce5e0ac3389c82/src/kimi_cli/auth/platforms.py)。
- [MiniMax OAuth contracts](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/oauth-core/src/contracts.ts)、[设备与刷新协议](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/oauth-core/src/oauth-client.ts)、[固定端点](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/oauth-core/src/endpoint-config.ts)、[managed 推理配置](https://github.com/MiniMax-AI/minimax-code/blob/da22f11c55d4748155d7162c1030414228a00565/packages/config/src/config.ts)。
- [GitHub 正式支持 OpenCode 的公告](https://github.blog/changelog/2026-01-16-github-copilot-now-supports-opencode/)、[OpenCode device flow 与传输](https://github.com/anomalyco/opencode/blob/32b57015e5b491a789fba1534ecb472e676c9d8e/packages/core/src/plugin/provider/github-copilot.ts)、[模型目录协议](https://github.com/anomalyco/opencode/blob/32b57015e5b491a789fba1534ecb472e676c9d8e/packages/core/src/github-copilot/models.ts)。
- [Qwen 当前快速入门](https://github.com/QwenLM/qwen-code/blob/main/docs/users/quickstart.md)、[iFlow 关闭公告](https://github.com/iflow-ai/iflow-cli/blob/main/README.md)、[Gemini CLI FAQ](https://github.com/google-gemini/gemini-cli/blob/main/docs/resources/faq.md)。

本轮脱敏证据在项目外的 `review/oauth-live-flow-20261025/`，包括源码固定 commit、device-start 真实 HTTP 结果、MiniMax 公共快照。授权用户码、设备码、完整验证 URL、用户 token 不进入该证据目录。真实授权完成与付费推理必须分别记录实际结果，不能以设备端点 200 替代。
