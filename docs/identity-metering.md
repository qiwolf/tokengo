# 路线 B：多用户、VKey、额度与计量 API

本文记录路线 B 初始开发阶段的身份与计量合同，沿用 Rust/Axum/SQLite；当时没有部署或迁移现网。当前分发与部署证据见[验收状态](acceptance-status.md)，不能将本页阶段性说明理解为当前仍未发布。

## 身份与授权

旧 key 默认属于 `users.id=1` Legacy 用户，原始 key/ID 不变。原有设置/可复制 key API 保留兼容；新 `/api/users/{id}/keys` 与 `/v1/keys` 创建的高熵 key 只存 SHA-256 摘要和提示，完整值仅创建返回一次。数据库加密配置仍保护旧兼容 key 和上游凭据。

AuthContext 包含 user/key/role、模型/来源/费用及限流策略、父链最早到期时间和 SHA-256 策略版本；请求/尝试/金融账保留当时的身份快照。每次认证都读取当前用户、key 和祖先 key：禁用、撤销、过期、父链缺失/跨用户/循环均拒绝。认证缓存只缓存凭据验证，不缓存权限；冷 Argon2 验证结束也重新比对当前凭据和身份。新 SHA key 不需要 Argon2 阻塞池。

用户策略与本 key/全部父 key 策略取交集：

```json
{
  "allowed_models": ["deepseek-v4.1-flash", "auto"],
  "allowed_channels": [1, 2],
  "allow_paid": false,
  "rpm": 30,
  "tpm": 60000,
  "max_concurrent": 2,
  "admin_access": false
}
```

省略白名单（null）继承不额外限制，空数组拒绝全部；模型归一化比较。`auto` 本身及实际候选模型都必须获准。别名不会绕过来源白名单，fallback 每次尝试重新读权限。不可用路由的错误响应也只列出获准的来源名称。来源是否付费按该渠道/逻辑模型当前价格判断；无价格按可能付费处理，禁止付费的 key 不会把未知价格当免费。

普通用户通过自己的 vkey 访问自助 API，无需共享面板密码。自助列表/撤销限定当前 key 及其后代，分发给应用的窄 key 无法枚举或撤销父 key、兄弟 key；全局管理由管理员入口完成。自助创建的 key 是父 key 的子 key，不能扩大策略、延长超出父链的有效期；滚动 RPM/TPM 用量和并发也共同限制所有子 key，避免建新 key 绕限。`/v1/usage` 限定当前 key，不能通过 query 读取别人账单；用户可从自己的各 key 查看其用量，管理员可按 user/key 汇总。

原有 PanelAuth 是 root 管理入口。新路线 B 管理 API 同时支持管理员 vkey，但须用户角色 admin，且用户策略及 key/祖先策略显式开启 `admin_access`。默认 false，历史 gateway key 不会自动获得管理能力。普通 user 即使策略字段为 true 也不能调用管理接口；管理员 VKey 不因此取得渠道或完整全局设置管理权限。独立[运维令牌](operations-tokens.md)是另一类授权，可访问 `/api/ops/capabilities` 明确登记的业务管理动作；完整设置写入、root安全和历史密钥明文查看仍限root。

## API

| 接口 | 行为 | 权限 |
| --- | --- | --- |
| GET/POST `/api/users` | 列表/创建用户（name、role、policy） | 管理 |
| GET/PATCH/PUT/DELETE `/api/users/{id}` | 读取/更新/禁用，保留历史 | 管理 |
| GET/POST `/api/users/{id}/keys` | 列表/创建所属 key | 管理 |
| PATCH `/api/vkeys/{id}` | 更新命名、策略、状态、有效期、撤销 | 管理 |
| GET `/v1/me` | 当前身份与实际策略 | 当前 key |
| GET/POST `/v1/keys` | 当前 key 及后代元数据/创建受父链限制的子 key | 当前 key |
| DELETE `/v1/keys/{id}` | 撤销自身或后代 key | 当前 key，归属与父链校验 |
| GET/POST `/api/pricing` | 列表/追加不可变价格版本 | 管理 |
| GET/PUT `/api/budgets?scope=user\|key&scope_id=N` | GET 读取历史消费并返回 budget_enforcement=false；PUT 返回 410 budget_enforcement_disabled | 管理 |
| GET `/api/usage` | 按 user_id/key_id/channel_id/model 筛选持久账 | 管理 |
| GET `/v1/usage` | 当前 key 持久账与历史统计（budget_enforcement=false） | 当前 key |

变更拒绝未知字段；身份变更与审计在同事务，审计不含密钥。新 key 创建响应 `{"key":{...metadata},"secret":"tk-..."}`，列表和用量不返回 secret。账本倒序游标 `before_id`，`limit` 1–200，返回 `entries` 与 `next_before_id`。

## 价格版本与费用记录（2026-11-14 当前合同）

新价格金额用整数微人民币（1 CNY = 1,000,000 micro）；历史 USD 原值保留，用量看板按展示汇率换算人民币，不重写账本。Token 输入/输出成本合计后向上取整一次，Search/Fetch 按次、ASR 时长等数量计价分别记录。配置价格不等于对应接口已实施自动计量，范围见[版本说明](releases/v0.34.0-qiwolf.32.md)。

三类价格均支持创建、修改、停用/恢复和软归档。修改追加不可变新版本；生命周期状态独立于金额行，最新版本停用或归档不偷偷回退旧价。默认列表隐藏删除项，历史可查询；已入账/在途尝试继续使用其原价格快照。未定价费用为 null，不等于免费，付费权限仍把未知价格按可能付费处理。

所有账户取消本地金额预算准入、现金 hold 和超估算冻结，旧 limit/frozen 字段不再拦截；用户/Key 写入的旧 limit_micro 兼容忽略。PUT /api/budgets 返回 410 budget_enforcement_disabled；GET 及 /v1/usage 返回 budget_enforcement=false，保留历史消费可读。权限、RPM/TPM、并发、到期和真实来源限制继续生效。

每个真实上游尝试保留身份、来源和价格快照、输入/输出/数量估算、实际用量及费用确定性。新现金 hold 为 0；已知实费完整记录，不封顶于估算，超估算可标记 overrun 但不冻结。缺失或部分 usage 保留已知分量，不能算出的实费保持 null/unknown，不把估算当作扣款。历史终态账本不重写；旧在途 hold 在结算/恢复时按各 allocation 原值释放，restart_unknown 也不把未知实费填成旧 hold。

金融账不随请求清理删除，终态结算幂等。已知结算先持久写入与数据库同目录的 <db文件名>.billing-outbox/，数据库写入失败由后台补写。启动先重放已知意图再收尾未知在途项；异库、损坏或冲突文件隔离到 dead-letter。备份/恢复须同时包含数据库一致性快照、主密钥与 outbox/dead-letter，不能丢弃补写队列。

## 来源界限与自动发现

来源 protocols 的成对正整数 max_input_tokens/max_output_tokens，以及现役 OAuth 约束优先。管理员可通过 POST /api/channels/{id}/limits/discover（root，body 为 {"model":"逻辑模型"}）读取该来源实际 Key 的目录声明；返回 limits、来源 URL、时间和声明状态。缓存按来源、模型和现役身份隔离，网络等待后重新核对 Key/映射身份；缓存与审计原子保存，不改费率或手工协议配置。未知不猜值，不把普通 API secret 当 OAuth 令牌使用。

有效来源窗口由手工配置/OAuth 优先，其次现役身份有效的供应方声明。来源参数与 Token 价格表单可以自动预填，管理员可调整；现有价格和手工值不被静默覆盖。目录声明并非付费推理或硬窗口探测。

价格输入/输出/数量参考值不是调用门槛。文本用保守 JSON 字节估算，已知来源输入窗与实际 token usage 分开。原生不透明历史/音频可使用来源窗口或价格参考作估算；无 TPM 且两者未知时仍可 JSON 估算转发。TPM 启用时需保守预占，排队后的实际估算不能超过已入场令牌 hold；真实来源窗口、视觉/音频权限和参数合同独立校验。会话标识不固定来源，供应方仍可拒绝异源加密历史，见[协议](protocols.md)。

## 请求/token/并发限制

用户、本 key 及所有祖先 key 同时生效。RPM 与 TPM 使用持久的滚动 60 秒窗口，TPM 预占输入上界+最大输出，不因取消退款，防止取消/重启规避限额。已限制请求拒绝不会部分占用另一 scope。并发槽涵盖排队和整个 SSE body 生命周期，完成/失败/取消均释放。

本批并发槽在单个 AppState 内维护；部署拓扑限定单进程/单实例管理此 SQLite。未实现多实例共享并发控制，不能将其当作集群额度服务。

## 验收边界

有受控 HTTP 与真实 SQLite 并发/迁移回归，不代表真实供应商账号、客户端订阅或生产行为验收。系统管理员原有探测/诊断接口仍属于 root 运维路径，并未自动摊入某个普通用户账单。当前也提供 Responses/Messages 入口与用户/VKey、路由、账务管理页面，协议及独立 OAuth 的限制分别见[协议说明](protocols.md)和[OAuth 文档](openai-oauth.md)。Search/Fetch 使用独立请求单位和价格，不占用 token 配额，见[按次计费说明](deepseek-search-fetch.md)。不据本地测试推定真实订阅授权、供应商价格或完整客户端兼容性。
