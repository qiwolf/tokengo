# G01–G20 核心验收逐项审计

审计基线 `4d91f79`；2026-10-28更新本批验收收据。本表是验收索引及缺证清单，不改写研究时刻的 gap-ledger 状态。历史测试日志中实际出现 `... ok` 的记录已抽取到仓库同级 `review/provider-p1-batch4-20261027/w11/historical-test-observations.json`；这些是旧源码验收，不是本批新源码通过声明。当前批次结果以新收据为准。

四层分别为官方文档、供应方形态、网关模拟上游、真实来源；有文档/fixture不能提升为真实来源。问题单状态已重新从GitHub官方API读取到 `docs/references/provider-compatibility-20261027/w11-issue-observations.json`，关闭/合并均不证明已发布修复；16条均未作为TokenGo真实供应方复现。

本批最新服务端/构建快照为phase13，清单SHA-256 `2cee5211fa646d4c8f8d25c2921cc4184e3467091cfbb9003e812600fe16f0bb`。build-test受影响范围699通过、0失败、2忽略（20组）；Web171/0/0、WASM/Trunk及live fixture编译退出0。phase11另有协议67、HTTP52、OpenAI SDK24对照、Anthropic SDK3对照均通过；不能与重叠测试简单累加。phase6的1535/0/8是旧快照全量，不是最终全量。

硅基流动四个精确托管型号共14次实际调用均通过，范围见[第四批验收](2026-10-28-provider-p1-batch4.md)。browser13发现的焦点缺陷已修复，focus-fix14分轮真实行为及最终audit退出0，失败原始记录保留；发布/237部署另有独立收据。下表历史来源仍按历史收据标注，当前具体通过范围以第四批收据为准。

|核心项|实际代码/测试入口与已观察历史证据|能力边界、缺证及本批状态|
|---|---|---|
|G01 / W02 预算|`parameter_budgets.rs`、`request_plan.rs`；`responses_source_budgets::native_answer_caps_reserve_verified_total_and_settle_incomplete_once`，`native_unknown_budget_uses_price_estimate_but_unrepresentable_conversion_still_rejects`；P1 `final-affected-http.log`观察通过|精确百炼Responses区分答案与总量；按2026-10-28裁决，native缺硬上界证据可用source/Price/4096本地估算，不虚标上游hardcap；min16、真实跨协议cap冲突仍为具体请求校验。phase13 native_reservation_estimates 4/0通过，W10合法未知native最小调用与本地估算HTTP通过；无百炼官方账号实测。|
|G02 / W03 opaque归属|`protocol_sessions.rs`；`chat_opaque_ownership::chat_opaque_json_and_sse_replay_preserve_and_reject_foreign_values`、`chat_opaque_replay_rejects_changed_source_generation`；P1 `final-affected-http.log`|JSON/SSE、错Key/会话/代次、跨协议负例已覆盖；本地所有权不依赖供应商拒签。Gemini/Ark官方账号未实测。|
|G03 / W01 Ark版本冲突|`contract_evidence::describe` 对Ark Chat返回 `required_before_new_capability`；P0收据记录冲突仍未解；`parameter_contracts::versioned_evidence_identity_does_not_grant_runtime_capabilities`历史通过|滚动模型真实修订/签名合同仍无账号证据，标未充分验收；不因此关闭整个合法原生接口。具体不可表达签名/转换请求仍拒绝；G19已补分层证据与身份，不补造原文hash。|
|G04 / W04 strict三态|`tool_strict_contracts.rs` 8项：omitted/true/false双向、native、nullable/nested及未知schema拒绝；P1 `final-affected-http.log`实际通过|不改schema，不确定严格化拒绝；Messages不能表达true保留拒绝。SF有效工具参数不证明供应商强制strict。|
|G05 / W05角色|`message_roles.rs`、`provider_roles_gateway.rs`；`provider_roles_gateway_outbound_text_order_and_fields` / `provider_roles_gateway_rejects_unrepresentable_messages_without_upstream`历史通过|官方Qwen/Kimi/GLM精确型号与SF实测合同分开。前缀折叠注明优先级语义；中途/多模态不可表示拒绝。本批SF增加DeepSeek-V4-Flash、GLM-5.2、Kimi-K2.7-Code三个精确托管型号各2次原生Chat JSON/SSE角色折叠；Qwen另有8次协议/工具往返。不泛化其他托管商或型号。|
|G06 / W08百炼域|`parameter_contracts::w08_workspace_endpoints_are_exact_and_manual_profile_wins`，batch3 `w08-green.log`实际通过|六区域单workspace label和明确路径，manual profile优先；近似域unknown；无百炼工作区账号真实调用，不把识别域名当授权。|
|G07 / W06 DeepSeek|`parameter_contracts_china::deepseek_native_responses_rejects_ignored_intent`、`deepseek_chat_forcing_requires_explicit_off`、`provider_parameter_combinations::deepseek_strict_default_does_not_escape_to_unverified_beta_transport`；P1合同/HTTP日志|原生Responses与Chat转换分线；不支持状态/被忽略控制拒绝；beta strict不逃逸来源。官方新Responses真实来源未验。|
|G08 / W06百炼工具组合|`parameter_contracts_china::qwen_tool_controls_are_model_and_mode_specific`、`provider_parameter_combinations::china_parameter_rejection_precedes_upstream_and_reservation`；P1历史通过|thinking/required/named/parallel按实际托管模型，不降级auto。百炼真实账户/版本未验。|
|G09 / W06 Kimi Responses|`kimi_summary_history::native_kimi_json_and_sse_complete_output_replays_anonymous_and_explicit`、`summary_proofs_are_separate_from_ciphertext_and_bound_to_complete_snapshots`；`kimi_responses_is_separate_from_chat_k3_and_k2`，P1日志通过|原序output、summary与cipher分开、错身份/变造拒绝；Chat K3 required与Responses auto分开。未知状态执行器保持拒绝，官方Kimi账号/原生失败形态真实捕获未验。|
|G10 / W07 usage|`provider_usage_metadata::kimi_cache_subsets_json_are_conserved_and_raw_usage_retained`、`partial_sse_usage_keeps_unknown_buckets_and_validates_against_known_input`、`kimi_sse_final_usage_duplicates_preserve_subsets_without_double_counting`；batch3 `w07-green.log`|cache/reasoning是总量子集；unknown不伪造零；原始usage有界响应内留存。现有billing幂等测试可复用，无供应商账单对账/持久化raw仓库承诺。|
|G11 / W06 GLM产品线|`parameter_contracts_china::glm_format_tools_and_product_line_are_explicit`、`parameter_contracts::p1_anthropic_mythos_and_coding_lines_have_exact_contracts`；P1合同日志通过|公共API/Coding Plan分开、不跨线路借none；新型号精确表。无官方不同产品线账号实测。|
|G12 / W07 GLM元数据|`provider_usage_metadata::glm_request_id_is_typed_metadata_and_semantic_extensions_stay_rejected`、`glm_sse_semantic_extensions_are_rejected`、`sse_partial_subset_conflicts_and_trace_changes_fail_closed`；batch3 `w07-green.log`|request_id有界类型化，不等同响应/会话身份；本批Messages tokengo_metadata及早/晚SSEtrace往返由phase13 provider_trace_messages 4/0、usage 8/0与phase11 Anthropic Python2.3.0三例对照通过；晚到trace仅声明事件可见，不声称SDK终态自动合并；搜索/过滤/媒体未知语义仍拒绝，不修改cache/usage门禁。官方GLM账号未实测，SF托管另记。|
|G13 / W06 GLM交叉约束|`parameter_contracts_china::glm_format_tools_and_product_line_are_explicit`，P1 `contracts-final-green.log`|required/named/schema/tool_stream及enum/text冲突保留限制；不把未核验组合变支持。真实产品线工具流待账号。|
|G14 / W06 Anthropic|`parameter_contracts::p1_anthropic_mythos_and_coding_lines_have_exact_contracts`、`anthropic_modes_budgets_and_forced_tools_are_not_equivalent` / `anthropic_sampling_rules_use_actual_api_version_and_preserve_native_extras`|精确型号与API版本、adaptive/legacy/forcedtool/warmup分开；账户权限未知，签名/思考形态不得从品牌前缀推断。官方账号真实覆盖缺失。|
|G15 / W06 MiniMax默认|`parameter_contracts_china::minimax_models_and_protocol_defaults_are_not_collapsed`、`parameter_contracts::p1_minimax_effort_bridge_keeps_native_levels_and_modes_separate`；P1日志通过|M3 Chat omitted开、Messages omitted关，M3.1独立；显式优先。无法无损转换保持拒绝，不假定服务方接受strict。|
|G16 / W09 MiniMax方言|`minimax_split_gateway::w09_http_and_sse_tools_replay_complete_raw_history_and_neutral_metadata` / `w09_explicit_nosplit_and_signed_messages_fail_before_transport_native_stays_native` / `w09_policy_semantics_never_retry_or_switch_source_before_or_after_commit`；batch3 HTTP日志|明确API Key/精确M3型号Chat split↔Responses支持；no-split转换与signed Messages的具体不可表示形态仍拒绝；2026-10-28用户裁决后strict原值保留，不因缺实测关闭工具转换，enforcement标未充分验收；原生直通不声称已验证全部方言。官方账号未实测。|
|G17 / W08 runtime|`runtime_identity.rs`、`protocol_config::w08_offline_identity_is_bounded_unverified_and_invalidates_cache`；batch3 `w08-green.log`通过|操作者/标准API都是未验证声明；template/parser/generation digest缺项unknown，身份变化隔离代次。W10独立采集/权限/限时取消、Embedding/Rerank用途独立采集由phase13 HTTP通过；候选仍unverified、不自动保存；无真实Ollama/vLLM部署实测。|
|G18 / W11矩阵|新增 `provider_compatibility::versioned_provider_shapes_are_chunk_boundary_invariant` + `fixtures/provider_compatibility/v1`；现有 `protocol_conversion_faults`、`chat_opaque_ownership`、`provider_usage_metadata`与`provider_live_siliconflow`复用|本批9种合成形态×3种分块；SDK锁openai-python3.40.0，runner消费生产converter导出流对比raw HTTP。phase13矩阵通过，phase11 OpenAI Python3.40.0的24对照、Anthropic Python2.3.0的3对照通过，相关协议/fixture输入至phase13未变化；SF四型号14call绑定phase13重验通过。SDK样本不叫官方来源验收。|
|G19 / W01证据/字段处置|本批 `contract_evidence.rs` p1.4补精确identity/model/API匹配与分层源证据；`request_plan.rs`实际转换/source roles→参数→mapping defaults→参数→validate原子流水线，gateway转换/预算实际接入固定路径处置记录；新增4项identity/证据测试与4项RequestPlan行为测试|phase13服务端库552/0/2及受影响HTTP通过，具体范围见第四批收据。metadata_record_sha256只哈希已归档研究记录，document_archive_sha256缺原文时null，模型真实revision仍unknown；不补造文档或实测。字段状态只用spec六态，operator默认是effect，不另造支持级别；known模型原样保留的未知字段仍unknown，有负例。|
|G20 / W10诊断|phase13 provider_diagnostics 13/0、custom_probes 10/0；Web171/0/0；规则ID与none匹配、展示用途/执行种类分离、合法unknown原生估算、OAuth/工具/Embedding/Rerank探针入口|静态不联网/不改代次、总时限/取消/stale/脱敏、标准API候选不保存、真实重排协议结构与诊断账本均有HTTP证据。未充分验收不关闭合法请求；新机器权限不自动授予。focus-fix14目标视口/键盘、metadata和三种探针真实执行及最终audit已通过，临时环境已清理；62个Web输入逐文件匹配发行提交。|

## W11矩阵与复用边界

版本化fixture元数据记录 vendor、authentication_lane、exact_model、native/client protocol、API/schema revision、SDK version来源和issue_reference。合成通用形态的 vendor 明确为 `openai_compatible_shape_not_live_vendor`，不能贴上Gemini/Kimi等标签伪装捕获。ISSUE-01关联只是“首帧同index片段”的触发经验，不宣称重现其服务端finish/index缺陷。

新增形态包括空工具数组、首帧同逻辑index的两段参数、末choice/独立尾帧和重复usage、partial usage、length incomplete、finish后EOF、无终态EOF、截断工具JSON与流内错误。生产converter在逐字节、7字节和完整块三种输入下必须给同一结果；错误不得制造成功终态。身份归属/参数拒绝/账务/提交后不切源已有网关HTTP测试，直接引用，不为矩阵重复造同一套数据库测试。

SDK runner仅使用loopback HTTP消费Rust converter真实导出的Responses流，比较事件末态、文本、工具call ID/name/JSON参数、输入输出usage和SDK发出的请求体；锁文件保留PyPI精确版本、wheel哈希、声明依赖，执行结果另记录实际安装依赖版本。它不证明SDK主动验证服务方schema，也不证明第三方客户端已经采用该版本。本轮对照包含Responses/Chat的JSON与SSE共24组合；本轮另有Anthropic Python2.3.0对JSON及早/晚SSEtrace的3例loopback对照通过，仍非真实Anthropic来源验收；不能把issue中SDK1.7.0当本轮运行版本。

```text
TOKENGO_PROVIDER_FIXTURE_EXPORT=<隔离输出目录> cargo test --locked -p tokengo-server --test provider_compatibility -- --nocapture
<build-test隔离venv>/bin/python crates/server/tests/fixtures/provider_compatibility/sdk_compare.py --exports <同一目录> --output <sdk-result.json>
```

仅在build-test执行；由root同步源码、核对版本/哈希、固定wheel安装、取回日志/产物。合成测试用例不读取SF Key，不访问供应商，不修改真实配置。本表不替代发布/部署收据，GitHub和237实际版本仍由W12各自确认。

## 明确排除同步承诺的可选接入

方案及计划原文将G21–G29/O01–O09明确列为可选接入，依次为xAI、Mistral文本、Mistral connector、Cohere V2、OpenRouter、Gemini原生、Azure、Vertex、Bedrock；这些是新接入项目，不是当前20项核心里被隐藏的修复。均保留 `proposed_not_implemented`，本批没有为其解除执行器/来源/预算门禁。

## G19当前实现与仍须诚实保留的未知

RequestPlan的记录来自实际执行的转换、角色、参数适配、默认值、最终验证和预算阶段。请求在私有副本运行整个来源流水线，失败不会留下半改的调用方body；gateway只接回成功副本，再进入原预占和发送路径。处置报告仅包含固定字段路径、阶段、规则和影响说明，debug记录不含消息、值、端点或凭据。原生未知字段保持unknown，整体validator通过不会提升每个保留字段为supported_native；只对明确字段验证的百炼Responses输出cap报告本地支持。显式none到文档off控制由实际adapter标exact_transform；角色优先级和MiniMax控制层折叠标documented_semantic_fold。

`contract_evidence::describe`保留有界model、typed protocol、安全API版本、endpoint family及鉴权产品合同线，OAuth类型由runtime binding核实，不能靠一个bool宣称managed账号权益。官方证据和形态/HTTP/真实来源四层分开，后面三层须独立收据；registry不授予能力、不改变binding_generation。实际source JSON只解析一次。未知未来模型/API版本不借旧精确规则，model-independent归属保护与端点识别仍可显示其保护范围。

本批没有补抓全部官方原文。可定位的原研究source record使用独立metadata hash；原文archive hash仅在已有档案确实提供时使用。文档revision和上游模型真实revision未获得证据时分别unknown，精确model ID不能当真实滚动revision。Ark冲突继续标未充分验收；不能借元数据修订宣称真实来源已验证，也不因此关闭整个合法原生接口。

错误处置同样来自实际拒绝点：`prepare_source_detailed`/`prepare_conversion_detailed`返回独立typed报告，HTTP错误继续使用原有message。错误报告只包含固定stage/路径；实际转换拒绝后台执行时用 `/background` 的executor_gated，角色、参数、默认值或最终验证失败用unsupported。`detailed_rejection_reports_are_typed_secret_free_and_atomic`覆盖失败后原请求不变、callback错误正文不进报告及真实converter拒绝background的门禁。旧Result<String>入口作为兼容包装保留。

最终提交全量CI：`f85eca9d67801a970ba93312a553da602deacea3`，[Rust/WASM CI 40559710708](https://github.com/qiwolf/tokengo/actions/runs/40559710708)成功，Rust1560/0/9、Python13通过。真实供应方与SDK证据仍按上文范围记载；完整发布和237版本见[发行收据](2026-10-28-qiwolf22-release/README.md)。
