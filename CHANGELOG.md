# 更新日志 / Changelog

本文件记录本 fork 的变化。Cargo 版本仍沿用上游 **0.34.0**；下列 Unreleased 不代表已发布新 tag、预编译程序或完成生产升级。服务端与 Web 资产须使用同一审阅提交构建。上游历史请参阅原项目发布记录。

## Unreleased — 2026-10-20

### 服务商渠道、模型与原生参数

- 增加稳定 ID 的服务商父层，原渠道作为独立连接保留；凭据、协议、OAuth 绑定、并发、价格、fallback、VKey 来源范围和历史继续引用原连接 ID。支持未分组连接明确归属，不根据域名或名称自动合并。本轮六组是配置方案，不是硬编码数量限制。
- 增加连接模型非破坏启停及指定多连接范围的原子开关；停用保留上游映射、视觉能力与字段配置。模型页默认汇总公开、启用、可路由项，停用项和兼容隐藏项可显式查看/恢复。
- 标准 GPT 展示使用 `gpt-6.1-sol`、`gpt-6-luna`、`gpt-5.6-luna`，以授权账号目录为准。旧十个档位别名可保持隐藏兼容，显式请求及历史不被删除，不按后缀自动改名。
- 从普通目录及 OAuth 官方目录读取实际推理 levels/default/source，新增模型缺缓存时只获取元数据。boolean 支持不推断档位；未知可由操作者明确人工确认。每逻辑模型默认档位与模型名分离，客户端原生参数优先。
- Chat↔Responses 处理可表示的 `reasoning.effort` / `reasoning_effort` 与 `service_tier` 请求字段。目录声明不等于推理实测或全部跨协议兼容；签名、加密与不透明推理仍受显式限制。
- 固定旧视觉检测的持久指纹合同：服务商分组和新增模型开关不会误清已有退避状态；真实来源、上游映射或视觉能力改变仍使指纹失效。
- 修复手机表格字段标签、公开计数包含兼容隐藏项、连接保存后父服务商状态未更新及已归属连接显示假“未分组”选择框的问题。

### 账号登录与控制台

- 独立管理员/普通面板账号使用账号密码与同源 HttpOnly cookie。既有管理员原哈希迁入，初始名 admin；本人修改登录名或密码须确认当前密码并撤销全部会话，旧凭据失效。
- 普通账号明确绑定 API 用户，仅查看本人安全密钥元数据和用量；面板停用与 API user/VKey 停用是不同操作。root 不能通过普通账号管理入口被禁用或绕过当前密码重置。
- 首次设置保留 Gateway Key 保存确认步骤，不将登录 secret 写入 localStorage；修复可选显示名留空被错误提交的问题。
- 更新登录、本人门户和共享控制台外壳，支持桌面/手机、浅深色、可见焦点、自动填充和关联错误；不声称已全面重设计业务页或完成全设备无障碍审计。

### 已有网关与账务能力汇总

- 多 API 用户、多 VKey、子密钥/祖先权限与配额、付费来源控制、预算、版本化 token/request 价格及持久逐尝试账本。原子预占与结算 outbox 支持恢复；未知用量/未定价保持未知，供应商发票另行对账。
- 逻辑模型有序 fallback、受限重试、冷却、并发与取消处理。流式提交前可切换允许来源，提交后不重放工具或输出。
- 原生 Chat / Responses / Messages、受限文本/客户端函数工具转换、向量与重排；显式来源能力约束图像、原生内联语音及不透明状态。
- 独立 DeepSeek Search/Fetch 与按次入账；独立 OpenAI OAuth 注册、加密凭据轮换、目录与订阅来源。OAuth 预览有独立参数限制，不等同普通 API Key 或任意客户端完整兼容。
- 真实指标、运行进度、分组明细与账务看板；完整成本趋势图、通用 BI 或多实例共享配额不在现有交付声明内。

### 验证与部署状态

本分支已实现以上功能。服务商/档位改造已有隔离目录、HTTP 与浏览器行为验证；有限真实 DeepSeek Search/Fetch、原生 CLI 及 OpenAI OAuth/指定转换路径的历史验收，不被扩大为全部供应商、模型、参数或客户端功能均通过。具体合同见 [服务商渠道](docs/provider-channels.md)、[账号](docs/account-login.md)、[协议](docs/protocols.md)、[Search/Fetch](docs/deepseek-search-fetch.md) 与 [OAuth](docs/openai-oauth.md)。

生产验收状态见[公开验收状态与边界](docs/acceptance-status.md)；以最终精确提交 CI、配套构建资产和部署读回为准。本日志不记录临时运维流水，不把实现、合成测试或旧版本测试数量当作本轮上线证明。

---

English readers: this unreleased update adds explicit provider groups with preserved connection identities, non-destructive model switches, native reasoning metadata/defaults, standard model names with explicitly hidden compatibility aliases, and account/password cookie login with a scoped user portal. Existing VKey, fallback, durable accounting, protocol and standalone tool features remain subject to their documented limits. Catalog capabilities are declarations, not paid inference tests; limited real-provider/client acceptance is not universal compatibility. Production acceptance is tracked in the [public status](docs/acceptance-status.md), and Cargo remains 0.34.0.
