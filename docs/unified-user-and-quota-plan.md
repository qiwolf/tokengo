# 统一用户、自助 Key 与额度改造方案

日期：2026-10-20。状态：基于现有代码的分阶段改造方案。本轮仅交付协议回归、回退说明及用户/套餐/探针方案，**不实现本文件中的新用户、套餐或自定义探针功能**。

配套说明：[协议选路与桥接矩阵、回退逻辑图和自定义探针合同](protocol-routing-matrix.md)。自定义探针的字段、执行与响应断言合同以该文档为准，本文仅补充权益与计费边界。

## 结论

用户的建议成立。现有代码把登录凭证和 API 权益主体分表是合理的安全边界，但把两者暴露为两套要人工关联的“用户”是不必要的产品负担。建议改造为一个“用户”入口：管理员一次性创建登录账号、授权模型和额度；用户登录后查看自己的权益、创建和撤销自己的 API Key、设置 Key 子限额。

内部保留 `users.id` 作为不变的权限与账单主体，`panel_accounts` 作为凭证存储，不强行合并数据库表。API Key 不得因为账号改名、重置密码或 UI 合并而更换 ID、密文、哈希、selector 或 owner。普通用户永远不能从自助页面取得管理权限。

## 已证实的现状

|位置|当前行为与影响|
|---|---|
|`crates/server/src/db/accounts.sql:2-12`|登录账号另存 `user_id`；root 的 user_id 为空。当前没有普通用户绑定唯一索引，多个登录账号可能共享同一计费主体。|
|`crates/server/src/db/accounts.rs` 的 `create`|创建普通登录账号必须提供已存在且启用的 API 用户；没有一次创建两者的服务函数。|
|`crates/server/src/routes/accounts.rs` 的 `create` 与 `portal`|账号创建传 user_id；普通会话门户按该 user_id 筛选 Key 和账本，现有门户是只读。|
|`crates/web/src/pages/accounts.rs`|独立“面板账号”页面要求选择绑定 API 用户，并跳转另一页面管理 API 用户。|
|`crates/web/src/pages/users.rs`|用户策略和 Key 策略均为 JSON textarea，过期输入要求 Unix 毫秒。|
|`crates/web/src/pages/portal.rs`|列出 Key 标识和用量，提示新增密钥联系管理员；没有浏览器会话自助创建 Key。|
|`crates/server/src/routes/identity.rs`|已有 `/v1/keys` 持 Key 创建子 Key，限定本 Key 子树并保留父链；不能直接拿来当浏览器会话自助 API。|
|`crates/server/src/db/identity.rs:12-27`|已有模型/连接白名单、付费开关、管理能力、RPM/TPM/并发，提供 intersection 和 no_broader_than。|
|`crates/server/src/db/billing.sql`、`db/billing.rs`|现有预算是微金额 `limit_micro/spent_micro/reserved_micro`；账本按尝试记账，保留输入输出 tokens；尚无月度权益周期或原始 Token 总额度。TPM 只是分钟限流，不能充当 Token 余额。|

因此不能把页面改名后就声称统一用户完成，也不能把现有金额余额重新标成 Token 余额。

## 数据与迁移合同

### 第一批：统一创建、可视化权限、自助 Key

新增迁移的编号应按实施时最新 schema 递增，不占用当前 PR #3 的迁移编号。

1. `users` 新增 `expires_at_ms INTEGER NULL`、`self_service_keys INTEGER NOT NULL DEFAULT 0`、`max_active_keys INTEGER NULL`。历史用户保守默认不自动开启自助；管理员统一编辑可开启。新普通用户由统一创建 API 明确写入允许自助和有限 Key 数上限；默认值建议为 10，实施前写入产品配置和测试。`NULL` 到期表示长期；0 个可用模型表示明确禁止，不能被前端解释为全部模型。
2. `panel_accounts` 表、ID、密码哈希和版本保持原值。新服务层 `managed_users::create` 在同一个 `db::in_tx` 中创建 users、panel_accounts、初始权益和审计；用户名已占用/密码非法/模型非法/额度非法全部回滚，不留下孤立 user。禁止调用当前会各自开始事务的 helper 形成嵌套提交。
3. 不立即给旧 `panel_accounts.user_id` 加 UNIQUE。先离线生成只含 ID 的重复绑定清单。每个旧身份仍只能看到原 user 的数据；不自动复制、归并或拆分其账本。重复绑定主体标为“旧版共享登录”，保留原访问范围；新账号禁止选任意 user_id，后续人工决定共享身份是否拆分。任何拆分只影响未来新 Key 归属，不重写历史记录。
4. 无登录凭证的历史 users 显示为“尚未启用登录”，管理员在同一个用户详情内设置用户名/初始密码。不得猜测名称匹配，不自动生成并散发密码。已有一个登录凭证的 users 直接显示为一个用户。root 保留内部管理身份，展示在统一用户页面的管理员行；API 的 admin 角色不升级为 root。
5. 账号整体停用、到期：同时阻断会话和新 API 请求，撤销相关会话；保留 Key 和账本行，不能物理删除。登录凭证停用仅适用于内部维护，不作为普通用户页面的默认开关。密码修改仅撤销登录会话，不自动轮换用户的 API Key。
6. `gateway_keys.user_id/parent_key_id`、`billing_ledger`、`billing_allocations`、价格版本、旧策略、审计完整保留；只新增记录和列。已撤销 Key 永远不能复活；旧可回显 legacy Key 不向普通用户提供回显接口。
7. 迁移前后保护集：旧用户/Key/账本主键集合相同；所有原 owner/父链/哈希/价格/账本金额/用量/终态/时间戳逐行一致；没有凭证读取输出；对同一旧 Key 的有效模型/连接集合完全相同。迁移不重置既有余额或 reserved。

### 第二批：明确区分月套餐、按用量金额、原始 Token 额度

三个概念分别持有，不互相换单位：

- **按 Token 定价计费**：供应商/平台输入输出单价乘实际用量产生金额；现有 price_snapshot 和金额账本继续负责。不同模型单价不同，不能把 1 元等价于固定 Token。
- **Token 配额**：独立的 `input_tokens + output_tokens` 原始配额。缓存/推理 Token 若已包含于供应商 input/output 不再重复相加。usage 不明不写 0，也不伪称精确余额。
- **月套餐**：一份有起止日期的权益授予，可包含 Token 额度和/或金额额度；套餐价格是购买对价，与用户可用额度是不同字段。第一版是管理员发放权益，不声称已有支付/自动扣费/自动续费。套餐到期动作、剩余额度结转必须显式定义。

建议新增类型化表（所有数量和金额使用检查溢出的整数）。以下是草案，`carryover=none` 等周期约束仅展示所建议规则；确认计费口径后再定最终 schema：

```
entitlement_grants(
 id, user_id, kind CHECK kind IN('one_time','monthly'),
 valid_from_ms, valid_until_ms,
 recurrence_timezone, recurrence_anchor_day,
 token_limit NULL, money_limit_micro NULL, currency NULL,
 carryover CHECK carryover IN('none'),
 package_fee_micro NULL, package_fee_currency NULL,
 state, created_by_account_id, created_at_ms, version
)
entitlement_periods(
 id, grant_id, sequence, starts_at_ms, ends_at_ms,
 token_limit NULL, token_spent, token_reserved,
 money_limit_micro NULL, money_spent_micro, money_reserved_micro,
 state, UNIQUE(grant_id, sequence)
)
key_quota_limits(
 key_id, period_id, token_limit NULL, money_limit_micro NULL,
 token_spent, token_reserved, money_spent_micro, money_reserved_micro,
 PRIMARY KEY(key_id, period_id)
)
entitlement_allocations(
 ledger_id, period_id, dimension CHECK dimension IN('token','money'),
 reserved_units, debit_units NULL, certainty, settled_at_ms,
 PRIMARY KEY(ledger_id,period_id,dimension)
)
```

实际实施可拆 token/money 表以更严格约束，不可将微金额与 tokens 放进一个无 dimension 字段。币种不一致禁止比较或合并，不引入隐式汇率。旧 billing_budgets 保持原来的终身金额上限，与新周期上限共同生效。

**以下周期规则是建议，尚未经用户确认：**采用计费时区 Asia/Shanghai、自然月半开区间 `[月初00:00, 下月月初00:00)`，落库为 UTC 毫秒；界面显示精确时区/起止日。月中首次开通到下月月初为首个短周期，完整额度由管理员明确确认，默认不按天假装折算；续期计划只为未来周期生成新 period，绝不清零旧 spent。若产品选择按订阅日滚动月，使用独立 recurrence 类型，31 号钳位月末并保持原 anchor，不能在同一字段含糊处理。

建议不结转；若采用该规则，明确显示月套餐是“每月重置权益”，用户不能自行续期、补款或增额。套餐变更、追补、撤回均新增审计/版本，不覆盖过去已结算权益。第一版每个用户同一时刻只允许一个套餐 grant 激活，避免未定义多套餐扣除顺序；额外终身限额与该套餐取共同约束。周期边界并发创建使用 UNIQUE 和事务。

预占在发送前同时检查用户、Key、父链、终身金额和适用周期；tokens 预占使用可证明的输入/输出上界，预占不可靠时拒绝严格 Token 限额请求并提示配置，不靠猜测保证不超额。结算必须在同一个事务中终结 ledger 与释放/扣除所有维度，唯一 attempt_id 保证幂等。跨月请求归属发送预占时的旧周期，月底结算仍写旧 period；禁止按照结算时间改扣新月。周期回收不能删除尚有 reserved 的旧 period。

usage 缺失：保留预占或采用明示的保守估算状态，不把未知当精确扣款；平台对账后追加更正记录。失败前未发送可释放；已发送但结果未知不直接返还。超预占冻结/待对账，与现有金额账本一致。**待决定的客户计费口径：**Token 套餐扣除所有真实上游尝试的消耗，还是仅扣最终成功尝试，尚未约定，不能在实现时擅自选定。无论选择哪一种，供应商成本 ledger 始终记录各次真实尝试，现有历史账本不变；客户应扣权益应使用独立 allocation/客户账务视图，不能删除失败尝试或改写供应商成本来实现。UI 同时显示“客户端请求数”和“上游尝试数”，并公开套餐扣除口径。自定义探针的系统预算与费用归属放到第三批确定并实现；建议由系统/管理员承担并独立标识，不在未约定时记进用户套餐。

## 可执行 API 修改清单

新增 `/api/managed-users` 管理 API，旧 `/api/users` 和 `/api/accounts` 暂留兼容（停止在主 UI 暴露手动关联）：

|接口|契约|
|---|---|
|GET /api/managed-users|统一列表，含 user_id、可空 credential_id、显示名称、用户名、启停、到期、有效权益摘要、登录状态；不含密码/Key 秘密。|
|POST /api/managed-users|Root+CSRF，body 为 username/password/display_name/policy/expires_at/self_service_keys/max_active_keys/initial_entitlement；事务一次创建，不接收客户端 user_id 或 root role。|
|GET/PATCH /api/managed-users/{user_id}|Root+CSRF 更新身份、权限、到期、启停；凭证另有内部 ID，不通过可变用户名寻址。密码重置沿用现有会话撤销代码。|
|POST /api/managed-users/{user_id}/login-credential|为没有登录凭证的历史主体启用登录；不能把已有账号重新挂到另一个 user。|
|GET /api/account/entitlements|Browser 认证，仅从会话推导 user_id；返回当前/历史周期、单位、已用/预占/可用、未知项，不接受查询别人的 user_id。|
|GET/POST /api/account/keys|Browser+CSRF；列当前 user 的 Key 元数据；创建仅返回一次 secret，Cache-Control no-store；body 严禁 user_id/role/admin_access/parent_key_id。浏览器直接创建归属本人的顶级 Key，不依赖先拿一个父 Key。|
|PATCH/DELETE /api/account/keys/{id}|Browser+CSRF；WHERE id AND user_id，非本人一律404。允许更名、缩短到期、收紧权限/额度、停用、撤销；不能撤销后复活，不能提升上限。|
|GET /api/account/policy-options|仅返回本人允许的模型选项和安全策略边界，不泄露未授权连接详情或上游 Key。|
|POST/PATCH /api/managed-users/{id}/entitlements|Root+CSRF，显式类型、单位、币种、起止和版本；改套餐不允许客户端清零已用；未知字段拒绝。|

新自助创建服务在事务内再次校验当前会话/用户启用/到期、用户实时 policy 和 Key 数（并发创建不能越过 max_active_keys）。用户输入 policy 是 API 的类型化对象，不是给人写的 JSON 文本。显式请求宽于用户权限返回403，缺省按当前可授予范围构造；任何自助 Key 强制 admin_access=false。之后父用户缩权，即使 Key 旧 policy 没改也实时收紧。

Key 限额不等于独占预分配信用：多个 Key 共用用户总余额，各自子上限不能增加总权益。UI 的“额度管理”是查看用量、设定 Key 上限、撤销 Key；充值、套餐发放和提高用户额度仅管理员可做。用户不能通过创建多 Key、改名、撤销再创建规避用户总额。

保留 `/v1/keys` 持 Key 创建子 Key 的原树状安全语义，父链预算/期限/权限继续取交集；浏览器与 Bearer 混合凭证按现有规则拒绝。root 身份不借普通用户自助入口生成管理 Key。

## 页面点选合同

沿用清爽专业控制台风格，复用现有界面组件与视觉规范。统一管理员“用户”导航代替“用户+面板账号”两个入口，用户详情分基本资料、模型授权、额度/套餐、Key、用量/审计。root 的安全资料仍在账号安全页处理。

创建表单：登录名、显示名、初始密码、模型多选、到期日期、套餐/按用量配置、自助 Key 开关。成功后显示一个用户详情。模型授权支持“所有允许的当前及未来模型 / 仅所选模型 / 无模型”三种明确模式，不能把空选当无限。旧隐藏模型 ACL 要作为“兼容模型授权”保留选中和说明；不能因新 UI 模型列表不显示旧别名而在保存时删掉旧授权。

复用一个 `PolicyEditor` 组件供用户和 Key：模型 checkbox/搜索；付费模型开关；RPM/TPM/并发数字框配“继承/不限”状态；连接限制放高级区域（服务商分组→真实连接ID多选，勾选服务商只是对其当时连接集合的快照，新增连接不能悄悄扩权）；显示最终有效权限摘要。Key 自助选项只展示用户可用项，固定展示“受用户总权限和额度约束”。管理能力仅 root 管理页允许显式配置，默认关闭，普通用户组件不显示该项。

日期改成 date/datetime-local 并标注时区，API 继续用 UTC 毫秒；支持留空为继承/长期，但不能超过用户到期或父 Key 上限。数字字段区分未填、0、不限；Token 上限允许0表示禁止调用，与现有 RPM/TPM 必须正整数的规则分开验证。金额输入展示币种和小数精度，转换到整数微单位后再提交，不在浮点里累计。

普通用户空间提供“我的模型/额度”“API Key”“用量”三块：创建 Key 弹窗含名称、可选模型、到期、子限额；明文只在创建成功后的内存视图展示一次并可复制，关闭后无法再看，不写 localStorage/sessionStorage/URL/错误日志。撤销前提示影响该 Key 及后代。页面无需粘贴 API Key 才能自助管理。

用户不接触 JSON textarea。原后端 Policy serde 保留用于传输和存储；UI 读写应全部往返保持，包括 null/空集合/旧兼容模型/不可见连接/管理标志。遇到前端不认识的新字段显示只读警告并禁用有损保存，不能把解析失败替换成默认全权限。用户和 VKey policy 版本冲突返回409并要求刷新，避免旧页面覆盖管理员缩权。

## 三批实施与回归合同

以下是后续实施顺序，不表示本轮开始实现。每批分别提交可审阅改动和验收结果；前一批通过后再推进后一批，不能把整个方案混入当前协议测试提交。

### 第一批：统一用户、自助 Key、点选策略

- **数据/API**：新增 managed_users 原子服务及统一管理 API；保留 users.id 与内部 panel_accounts 凭证分表；新增 Browser+CSRF 的本人 Key API。不接入新的周期账务。管理员仍可配置已有金额预算，但必须标明单位，不能包装成原始 Token 余额。
- **UI**：合并“用户/面板账号”入口；用户创建后即可登录；完成模型授权多选、PolicyEditor、到期日期、Portal 创建/收紧/撤销 Key。无须填写 JSON，也不要求用户先持有一把 API Key 才能登录自助。
- **回归**：原子创建失败不留孤儿；旧 ID/Key/ledger 保护集；CSRF、混合认证、跨用户访问、管理权限自提权、父链缩权、账号禁用/到期、撤销不能复活、并发 Key 数上限、一次性秘密不进入浏览器持久存储；Policy null/[]/隐藏旧别名往返不损失；390px 手机和键盘可操作。

### 第二批：套餐与周期权益

- **实施前待决定**：自然月还是滚动月、计费时区、首个短周期权益、结转规则、套餐到期动作、Token 配额扣所有尝试还是仅最终成功、金额币种和客户计价规则。自然月 Asia/Shanghai、不结转只是本文建议。未决定的口径不得直接进入生产。
- **数据/API**：权益授予和不可覆写的 period、Token 与金额双维度 allocation、Key 子限额、版本化管理员授予/调整 API；保留旧终身金额预算，不清零历史 spent/reserved，不重写供应商成本 ledger。先确认已报告的高并发记账回归结论，再扩展新账务。
- **UI**：管理员按明确单位发放套餐和额度；普通用户查看套餐起止、可用/已用/预占、未知用量，并在用户总权益内管理 Key 子限额。第一版没有在线支付和自动扣款，不将管理员授予称为已实现支付订阅。
- **回归**：960 次并发请求的终态及守恒；原始 tokens、金额、TPM 不混淆；重复结算、跨月请求扣原周期、边界并发建 period、usage 缺失/取消/断连/fallback、重启未结算、套餐到期/变更、旧周期预占不被清理、重复 Key 不绕过用户总额。校验按最终确认的客户扣除规则；供应商实际成本始终完整。

### 第三批：自定义探针与系统诊断费用

- **API/UI**：按[自定义探针合同](protocol-routing-matrix.md)提供独立的探针配置、显式执行和断言结果；复用安全执行器，不另开绕过鉴权、计量或协议校验的 HTTP 客户端。内容和协议配置使用表单，保存与执行区分。
- **账务**：定义系统/管理员预算、探针来源标识、成本归属和取消/usage 缺失规则；默认建议与普通用户套餐隔离。OAuth、搜索/抓取及其他按次付费工具必须显式纳入执行和费用权限，不能因为“探针”默认视作免费。
- **回归**：真实执行形状、响应断言失败、超时/取消、输入输出上限、协议不兼容、频率与并发上限、失败成本保留、系统预算耗尽、未授权用户不能触发付费探针；自动探针和手工诊断明确区分，并验证不会改变既有用户额度。

每批发布前分别做隔离数据库迁移演练、保护集核对和必要回滚演练；文档明确本批交付与未实现项。无需为尚未实施的后续批次修改当前生产数据。

## 完成定义

完成全部相关批次后，管理员创建一个用户即可设置账号、授权和权益；用户登录后可自建和撤销 Key，以明确单位看到已用/预占/余额，不能自增权益或权限。旧用户/Key/账本 ID 与历史归属不变；用户和 Key 策略均不要求 JSON；手机可用。周期权益、自定义探针和相关费用只能在各自回归通过后宣称已支持。
