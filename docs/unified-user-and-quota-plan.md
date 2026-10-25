# 统一用户、自助 Key 与额度改造方案

规划日期：2026-10-20；以下实施沿革保留历史状态，当前功能与版本统一见[需求状态](requirements-status.md)和[验收状态](acceptance-status.md)。第一阶段统一用户、自助 Key 与点选策略，以及第三阶段自定义探针，均已实现，并已有匹配版本的浏览器、CI、分发与独立部署证据；历史qiwolf.7阶段的公开版本为 [v0.34.0-qiwolf.7](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.7)。月套餐、原始 Token 权益、独立系统诊断总预算仍是后续方案。历史阶段测试数不代表当前提交的全部测试结果。实际覆盖见[需求状态](requirements-status.md)，API合同见[账号与自助Key](account-login.md)。

配套说明：[协议选路与桥接矩阵、回退逻辑图](protocol-routing-matrix.md)与[自定义探针当前合同](custom-probes.md)。本文保留分阶段规划与尚未实施的权益草案，不作为新增用户字段或未来API已可调用的依据。

新增[通用 HTTP 透传与 MinerU 同步接入](passthrough-development.md)已随 qiwolf.6 交付，并复用既有 VKey、用户/父链约束及按次金额账本；它不新增月套餐或原始 Token 权益。新增 schema39 属于透传配置，不能视为下述周期权益草案已实施。透传配置和手动诊断目前仅 root 可用，不加入既有运维令牌90项动作。目标实例的合成解析及 Face/CLIP 字节对等已通过，既有客户端切流仍是独立工作。

## 结论

用户的建议成立。现有代码把登录凭证和 API 权益主体分表是合理的安全边界，但把两者暴露为两套要人工关联的“用户”是不必要的产品负担。第一阶段已改为一个“用户”入口：管理员一次性创建登录账号、授权模型和额度；用户登录后查看自己的权益、创建和撤销自己的 API Key、设置 Key 子限额。

内部保留 `users.id` 作为不变的权限与账单主体，`panel_accounts` 作为凭证存储，不强行合并数据库表。API Key 不得因为账号改名、重置密码或 UI 合并而更换 ID、密文、哈希、selector 或 owner。普通用户永远不能从自助页面取得管理权限。

## 改造前发现（历史基线）

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

上述是第一阶段实施前的缺口，不能再当作当前主流程。现阶段已新增原子管理入口、Browser自助Key与点选策略；金额余额仍是金额，不重新标成Token余额。

## 数据与迁移合同

### 第一批：统一创建、可视化权限、自助 Key

第一阶段实际实现与原草案有以下明确差异；它没有新增 schema，也不宣称已完成后续周期权益：

1. 新增 `routes/managed_users.rs`，在现有数据库原子事务中创建普通 users、panel_accounts、可选已有金额预算和审计，失败不留下孤立主体；管理操作在密码hash后再次复核管理授权；当前已登记接口允许root会话或独立运维令牌，完整权限以[账号合同](account-login.md)与[运维动作表](operations-tokens.md)为准。
2. 保留全部既有 users.id、凭证ID、Key摘要/owner/父链、价格与账本；历史无登录用户可在原详情补开登录，有凭证不能重复关联。旧多凭证共享用户不自动拆合，统一批量密码重置明确拒绝。
3. root保持独立安全身份，改登录名/密码沿用本人安全入口；普通用户/API admin不能由统一用户入口升级root。普通用户整体停用同步登录状态并阻断新API请求，密码重置撤会话，不轮换API Key。
4. **未新增用户到期、自助开关、可配置Key数字段。**浏览器自助创建固定最多32把活动Key，事务内计数；保留已存在Key，不批量删除旧Key。这个上限不等同历史管理员发Key接口的全局限额，也不意味着已有超过32把的用户被删除Key。
5. Browser+CSRF自助顶级Key从会话确定owner，不需先持VKey；策略只可在实时用户/旧Key/父链范围内收紧，admin_access强制false；金额子上限不能放宽，不能复活撤销Key。保留 `/v1/keys` 原有子Key语义。
6. 金额沿用整数microUSD累计/lifetime上限及既有spent/reserved，不新增原始Token余额或月度周期；新控件显式单位。Key到期使用页面日期控件，用户整体到期尚未实现。
7. 用户和Key使用共享点选PolicyEditor，保留null/空集合/隐藏旧值，未知字段阻止有损保存；**原方案的policy版本冲突409尚未实现**，不声称跨页面乐观并发保护。

定向回归覆盖原子创建/登录/自助、CSRF与归属、缩权、预算/父链、32活动Key并发、旧身份保全、凭证撤销/共享历史、授权目录与hash忙/根会话失效。浏览器完整操作、全工作区CI与独立部署已有分阶段证据；具体提交、测试数及边界见[验收状态](acceptance-status.md)，不以本段早期定向回归替代最终版本验收。

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

## 已实现 API 与后续草案

现有 `/api/managed-users` GET/POST、`PATCH /api/managed-users/{id}`、`POST /api/managed-users/{id}/login-credential` 为统一管理路径；`/api/account/keys` GET/POST及单Key PATCH/DELETE为普通浏览器自助。字段和安全合同以[当前账号API](account-login.md)为准，不接收原草案尚未实现的 expires_at/self_service_keys/max_active_keys/initial_entitlement 等用户字段。

旧 `/api/users`、`/api/accounts` 保留兼容，主UI不再暴露手工关联。未来 `/api/account/entitlements`、周期权益授予/调整 API 尚未实现，不应按这些草案调用当前服务器。所有Key仍共同受用户总权限、费用预算和原有父链约束，普通用户不能自行充值或提高总权益。

## 已实现页面与后续界面要求

管理员只有一个“用户”入口，统一填写登录名、显示名、初始密码、点选模型/连接/付费权限和RPM/TPM/并发，按需设置整数microUSD费用上限。历史无凭证用户在本详情补登录；root安全资料仍走本人账号页。普通空间显示本人的用量及Key管理，创建成功的secret仅内存展示一次，不写浏览器持久存储。

PolicyEditor明确区分无限/继承与空白名单，保留隐藏兼容模型和旧连接ID；输入未知字段阻止保存。自助Key编辑仅可收紧；Key到期使用本地日期时间控件，API仍使用UTC毫秒。多个Key共享用户总费用额度。当前没有用户到期、自助开关、月套餐配置或原始Token总额控件。

原草案关于用户到期、月套餐选项、Token子限额、周期可用/已用/预占、币种格式化及policy版本409的页面合同属于后续增强，不计入第一阶段已实现清单。手机390px与键盘操作须在实际浏览器中验证，单元测试和WASM编译不能代替。

## 三批交付状态与回归合同

第一批统一用户已有界面、CI、分发与独立部署验收；第二批月度权益仍未实现；第三批自定义探针已按[当前合同](custom-probes.md)实现并完成有限真实用途检查，不包含可配置系统总预算。各批单列可审阅改动及验证结果，不把未来方案算入本轮交付。

### 第一批：统一用户、自助 Key、点选策略

已实现上述无schema变更的原子统一用户管理、Browser自助Key、点选权限及已有金额子上限。早期定向服务端8项与Web端57项检查是历史验证记录，后续已有浏览器、完整CI、包/镜像和独立部署证据，详见[验收状态](acceptance-status.md)。创建用户→登录→建Key→模型调用→用量→撤销、手机/键盘操作、保护集核对及目标运行读回仍作为今后改变这些行为时的验收要求，不表示当前版本尚未发布。

本批排除用户到期/自助开关/可配置活动Key数、policy版本409、月套餐、原始Token权益与自定义探针。后续实现不得重写旧身份、费用或历史来补这些能力。

### 第二批：套餐与周期权益

- **实施前待决定**：自然月还是滚动月、计费时区、首个短周期权益、结转规则、套餐到期动作、Token 配额扣所有尝试还是仅最终成功、金额币种和客户计价规则。自然月 Asia/Shanghai、不结转只是本文建议。未决定的口径不得直接进入生产。
- **数据/API**：权益授予和不可覆写的 period、Token 与金额双维度 allocation、Key 子限额、版本化管理员授予/调整 API；保留旧终身金额预算，不清零历史 spent/reserved，不重写供应商成本 ledger。先确认已报告的高并发记账回归结论，再扩展新账务。
- **UI**：管理员按明确单位发放套餐和额度；普通用户查看套餐起止、可用/已用/预占、未知用量，并在用户总权益内管理 Key 子限额。第一版没有在线支付和自动扣款，不将管理员授予称为已实现支付订阅。
- **回归**：960 次并发请求的终态及守恒；原始 tokens、金额、TPM 不混淆；重复结算、跨月请求扣原周期、边界并发建 period、usage 缺失/取消/断连/fallback、重启未结算、套餐到期/变更、旧周期预占不被清理、重复 Key 不绕过用户总额。校验按最终确认的客户扣除规则；供应商实际成本始终完整。

### 第三批：已交付自定义探针与后续费用增强

- **已交付API/UI**：按[自定义探针合同](custom-probes.md)提供独立的探针配置、显式执行和断言结果；复用安全执行器，不另开绕过鉴权、计量或协议校验的 HTTP 客户端。内容和协议配置使用表单，保存与执行区分。
- **已交付账务**：系统诊断账本与普通用户额度隔离，记录来源、实际Token或请求次数、价格快照及取消/未知用量；OAuth和独立Search/Fetch已显式接入，不因“探针”视作免费。可配置独立系统总预算及其耗尽规则属于后续增强，不在现有版本中。
- **回归**：真实执行形状、响应断言失败、超时/取消、输入输出上限、协议不兼容、频率与并发上限、失败成本保留、系统预算耗尽、未授权用户不能触发付费探针；自动探针和手工诊断明确区分，并验证不会改变既有用户额度；其中系统总预算耗尽场景须待预算功能实施后增加，不是现有已通过用例。

每批发布前分别做隔离数据库迁移演练、保护集核对和必要回滚演练；文档明确本批交付与未实现项。无需为尚未实施的后续批次修改当前生产数据。

## 完成定义

完成全部相关批次后，管理员创建一个用户即可设置账号、授权和权益；用户登录后可自建和撤销 Key，以明确单位看到已用/预占/余额，不能自增权益或权限。旧用户/Key/账本 ID 与历史归属不变；用户和 Key 策略均不要求 JSON；手机可用。周期权益、自定义探针和相关费用只能在各自回归通过后宣称已支持。
