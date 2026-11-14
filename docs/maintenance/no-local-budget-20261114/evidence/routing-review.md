# Gateway选源复核

审查时点：2026-11-14北京时间。源码：`tokengo-usage-filters-20261107/crates/server/src/gateway.rs`，SHA256 `b8b0a8eefb7eddc5af20c13fdd0f851b93c981ac371a2a4a8e8c21ba46baa136`。本报告为静态代码审查，未在Mac运行构建/测试，也不替代root正在进行的build-test Linux验证。

## 结论

会话及历史affinity选源已清除。审查识别的完整400/429关闭切换绕过、缺省输出估算与最终默认不一致、pooled闸满不尝试其他可用Key，均已按root指令最小修复并补真实HTTP回归；另移除了无TPM时仅因未知费用估算而拒绝opaque/audio或估算漂移的残留门槛。代码已完成，须由本轮Linux回归确认。

## 已核对的当前行为 observed / derived

- Gateway全文搜索不再存在`attempt_order`、`affinity_token`、`ensure_affinity`、`switch_affinity`、`protocol_sessions::load/claim/verify/resolve/key/anonymous`及`ArtifactCollector`调用。仅保留`opaque_history`对输入结构的识别，供来源上下文预占使用；不读取或写入pin/artifact归属。
- 当前候选从`load_plan`读取最新路由/来源配置；1231-1241以plan当前顺序构造调用order。历史`routing.active`仍可写入统计状态，但不再被此处读取排序。直接来源顺序、虚拟目标顺序与每次请求的来源可用性筛选分开；自动模型选择及显式配置的向量round_robin继续按其现有策略执行。
- 218-306在同目标/provider/model组内读取当前credential priority/key_strategy及实时credential_load，ordered按priority/id，concurrent优先当前load，再priority/id，不读取历史用户affinity。
- 每个候选保留协议选择、来源能力、ACL、真实来源上下文和参数校验；排队后重新检查source generation、enabled/routable、凭据配置及Key授权。过期/撤销Key仍拒绝，不因取消会话锁源而扩大ACL。
- 723-748和752-767不再将price输入/输出上限作为拒绝请求的门槛；source_token_limit保留。价格仍能为未知语义的计量估算提供fallback，实际usage未知不造零价。独立工具缺价格使用reserve_unpriced_request，不再要求有价格才能调用。

## 已修复：关闭400/429切换仍可能继续下一个来源 [P1]

修复前：2820和3185附近仅automatic_target_failover或balanced_node才在advance_explicit_route=false时停止；普通pooled/private Chat可以落入后续账户/上下文错误分类或channel-fault路径，继续next candidate，绕过当前switch_off配置。

按root明确指令，本轮把两个完整400/429分支中`switched=false`统一停止并原样返回。没有恢复session/pool历史selector；既有明确vector_capacity拒绝在该分支前的例外保持。回归`tests/support/pool_virtual_cases.rs::explicit_400_is_request_local_and_disabled_switches_stop`保留原四组合，并增加400 context-length、429 insufficient-balance两个关闭切换的情景。该support包含于`tests/proxy.rs`。此项状态为代码已修，Linux结果待root收据。

## 已修复：省略cap时小price估算可间接阻断请求 [P1]

明确条件：未知来源输出语义（例如现有Hosted/New-Reasoner synthetic）、无source输出cap、price.max_output_tokens=2048、客户端省略输出cap。

静态数值路径：1346-1356初次admission对空cap投影按price估算2048；733-739在真正来源副本上物化默认4096；742-743再估算得到至少4096；2189-2191发现4096大于初次预占2048，返回source_output_changed并跳过来源。此时没有来源变化，小price仍成为间接门槛。

按root指令，初次admission的输出投影现在物化与最终请求完全相同的4096缺省值（依据真实source cap按choice收窄），managed OAuth保持其来源完整输出合同；price仍只作估算fallback。新增native_reservation_estimates::omitted_output_cap_uses_the_same_default_before_and_after_admission_despite_small_price_estimate，覆盖JSON/SSE、省略cap、price2048、无sourcecap，断言最终cap/ledger/snapshot一致为4096。

## 已修复：pooled非round_robin闸满会停在当前Key [P2]

1668-1700来源gate、1709-1744凭据gate，对pooled非balanced_node使用acquire_owned等待至request deadline，超时直接credential_busy；只有balanced_node（现向量round_robin）使用try_acquire并继续候选。

原路径影响“真实可用情况立即生效”：concurrent按raw load选中Key A（load1、cap1已满），Key B（load2、cap10仍可接收）时，会在A等待而不试B。按root指令，现两类闸均try_acquire，满时记录忙候选并继续既有顺序，全部忙才复用原有bounded queue；等待后若来源闸仍忙，释放临时凭据许可，避免反序等待。不引入新策略/开关。新增provider_pool::full_source_or_credential_gate_tries_the_next_available_key_before_waiting，覆盖ordered/concurrent×source/credential忙，其中并发场景保留A的满载许可及B的两个许可直至请求完成，以真实HTTP验证B仍可服务。

## 已简化：未知费用估算不能在无TPM时形成门槛

Generic opaque/audio无context/price时，inner保留JSON估算，不再仅因未知输入估算返回pricing_required。初次admission仅在当前用户/祖先Key确有TPM配置时要求最大输入计量依据；per-attempt input/output估算漂移检查也仅在tpm_limited时启用。真实source_token_limit、来源音频能力、协议形状、ACL、RPM/并发和有效期独立保留。Managed OAuth的真实来源上下文/default合同不改。

回归：protocol_safety::unpriced_opaque_history_requires_an_input_bound_only_for_an_explicit_tpm_limit，以及native_audio中无TPM未知费用200/有TPM缺输入依据403两例。未知价格保持unpriced/debit=None，不解释为免费。

## 验证边界

源码搜索与数值分支追踪为observed/derived；未声称Linux测试通过或生产部署。root正验证的旧green快照已因本轮P1修复作废，需同步新gateway/support回归后测试受影响suite并重新生成binary。后续源码变化须以新hash/新Linux收据更新本报告，不能把本报告hash当作未来最终版。

本轮仅在Mac编辑、做diff whitespace检查，未在Mac运行构建或测试；未commit、未执行生产配置写入。
