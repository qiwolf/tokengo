# 路线 B：多用户、VKey、额度与计量 API

本批沿用 Rust/Axum/SQLite，为后续协议/供应商适配提供身份和金融事实。没有部署或迁移现网。

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

普通用户通过自己的 vkey 访问自助 API，无需共享面板密码。自助列表/撤销限定当前 key 及其后代，分发给应用的窄 key 无法枚举或撤销父 key、兄弟 key；全局管理由管理员入口完成。自助创建的 key 是父 key 的子 key，不能扩大策略、延长超出父链的有效期；父预算、滚动用量和并发也共同限制所有子 key，避免建新 key 绕限。`/v1/usage` 限定当前 key，不能通过 query 读取别人账单；用户可从自己的各 key 查看其用量，管理员可按 user/key 汇总。

原有 PanelAuth 是 root 管理入口。新路线 B 管理 API 同时支持管理员 vkey，但须用户角色 admin，且用户策略及 key/祖先策略显式开启 `admin_access`。默认 false，历史 gateway key 不会自动获得管理能力。普通 user 即使策略字段为 true 也不能调用管理接口；敏感渠道/全局设置旧接口仍要求 PanelAuth。

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
| GET/PUT `/api/budgets?scope=user\|key&scope_id=N` | 读取/设置累计费用限额 | 管理 |
| GET `/api/usage` | 按 user_id/key_id/channel_id/model 筛选持久账 | 管理 |
| GET `/v1/usage` | 当前 key 持久账及预算 | 当前 key |

变更拒绝未知字段；身份变更与审计在同事务，审计不含密钥。新 key 创建响应 `{"key":{...metadata},"secret":"tk-..."}`，列表和用量不返回 secret。账本倒序游标 `before_id`，`limit` 1–200，返回 `entries` 与 `next_before_id`。

## 价格与费用预算

金额用整数 **微美元（1 USD = 1,000,000 micro）**。价格的输入/输出单价均为每百万 token 的微美元；成本按输入输出总数合计后向上取整一次，采用检查溢出的整数运算。

创建价格示例：

```json
{
  "channel_id": 1,
  "model": "deepseek-v4.1-flash",
  "input_micro_per_million": 1000000,
  "output_micro_per_million": 2000000,
  "max_input_tokens": 32768,
  "max_output_tokens": 4096
}
```

这只是示例费率，不代表实际供应商价格。价格行不可改/删，更新追加新版本；每次尝试保存当时完整快照，历史不会随新价格变化。

预算 PUT 必须显式提供 `{"limit_micro":1000000}`（或 null 表示无限），不修改已花费用，不重置历史。本批是累计/lifetime 限额，增加 limit 即充值；尚无日/月周期重置。用户与 key/祖先预算在同一 SQLite IMMEDIATE 事务中预占（金融写入连接启用 synchronous=FULL），任何一层不够则整体回滚，上游尚未发出。没有配置价格但任一层有费用上限时，返回 403 pricing_required。

每次上游尝试（包含 retry/fallback）单独预占并结算；某候选超费用预算或未配置必要价格时只跳过该候选并审计，可继续尝试获准且付得起的来源，不污染渠道健康。全部候选都被额度/价格条件拒绝才返回 429/403；不会用最后成功掩盖前一来源耗用。请求、尝试与金融账都有 user_id/key_id，金融账额外保存渠道、逻辑/上游模型、配置 revision、来源名和无用户信息/路径/查询参数的 origin、价格与身份快照、用量确定性；后续修改渠道不会改写历史来源。无价格且所有层无限的兼容请求记为 unpriced，费用/debit 都是 null，不能当作已知免费。

文本输入采用序列化请求字节数加保守开销作为 admission 上界；输出采用客户端显式 cap，缺省每个 choice 4096；Chat 的 n 必须为 1–128 的整数，TPM/金额均按 n 倍总生成上界预占，已定价请求还受价格配置 cap 限制并向上游补充 max_tokens。图像预占整项配置输入上限。未知 tokenizer、隐藏推理或供应商不遵守 cap 仍可能超出声明，本网关不能限制供应商实际账单。完全已知 usage 按价格结算；缺失/部分 usage 保留其已知分量并保守消耗预占，标记 estimated。观测成本超过预占则保留真实成本，客户扣款不超已预占，标记 overrun 并冻结所有相关用户、本 key 及祖先 scope（即使 limit 为 null，取消限额、免费或无价来源均不能绕过），需管理员核对配置与真实账单。不得把估算、unpriced、网关扣款和供应商账单混为一谈。

金融账不关联可级联删除的诊断行，30天请求清理不删除费用历史。终态结算幂等。已知结算先原子写入并 fsync 到与数据库同目录的 `<db文件名>.billing-outbox/`，数据库结算失败由后台指数退避补写（1–30秒）。启动先重放已知意图再对未知预占做 restart_unknown 收尾；损坏、异库或冲突文件隔离 dead-letter，阻止未经核对的启动恢复。备份/恢复必须同时包含数据库、WAL处理后快照、主密钥及 outbox/dead-letter。不要只复制 SQLite 文件后丢弃补写队列。

## 独立来源上下文上限

普通来源可在渠道 `protocols` JSON 中配置成对正整数 `max_input_tokens` / `max_output_tokens`，无需以零价格伪装成免费。原生不透明历史或音频使用已声明来源的完整输入上限保守预占；如果同时有价格且其上限小于该完整来源上限，则拒绝请求，不用较小价格上限低估预占。图像仍须满足独立视觉能力声明。

没有独立上限时保留原来的价格上限路径；两者都缺失时不接受需要完整上下文估算的输入。未定价来源若受费用预算限制仍拒绝，只有无限费用预算下才可保留 unpriced/null 费用继续执行。OAuth 来源继续使用其单独绑定的已核验上限，不以普通源声明替代；详见[协议与来源说明](protocols.md)。

## 请求/token/并发限制

用户、本 key 及所有祖先 key 同时生效。RPM 与 TPM 使用持久的滚动 60 秒窗口，TPM 预占输入上界+最大输出，不因取消退款，防止取消/重启规避限额。已限制请求拒绝不会部分占用另一 scope。并发槽涵盖排队和整个 SSE body 生命周期，完成/失败/取消均释放。

本批并发槽在单个 AppState 内维护；部署拓扑限定单进程/单实例管理此 SQLite。未实现多实例共享并发控制，不能将其当作集群额度服务。

## 验收边界

有受控 HTTP 与真实 SQLite 并发/迁移回归，不代表真实供应商账号、客户端订阅或生产行为验收。系统管理员原有探测/诊断接口仍属于 root 运维路径，并未自动摊入某个普通用户账单。当前也提供 Responses/Messages 入口与用户/VKey、路由、账务管理页面，协议及独立 OAuth 的限制分别见[协议说明](protocols.md)和[OAuth 文档](openai-oauth.md)。Search/Fetch 使用独立请求单位和价格，不占用 token 配额，见[按次计费说明](deepseek-search-fetch.md)。不据本地测试推定真实订阅授权、供应商价格或完整客户端兼容性。
