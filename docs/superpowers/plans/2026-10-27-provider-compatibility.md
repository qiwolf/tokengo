# TokenGo 多厂商兼容治理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Check off each item only after its behavior is verified; preserve the user’s existing authorization boundaries.

**Goal:** 补齐 20 项核心兼容工作，使来源合同、请求转换、预算、响应和会话归属可核查；9 项可选接入另行决定。
**Architecture:** 在现有 Rust 合同、转换器和会话保护上增量修改，纯数据规则与运行时执行分离，先交付 P0 修补，避免重写网关。
**Tech Stack:** Rust workspace、现有 Web 前端、JSON/SSE、现有数据库/计量；默认 build-test 构建测试，Mac 编辑及 Git。
**Spec:** [方案书](../specs/2026-10-27-provider-compatibility.md)

状态：2026-10-28核心W01–W12按授权批次完成实现、分层验收、qiwolf.22公开发行及237整包升级。证据见[核心审计](../../acceptance/2026-10-27-provider-core-audit.md)、[第四批](../../acceptance/2026-10-28-provider-p1-batch4.md)及[发行收据](../../acceptance/2026-10-28-qiwolf22-release/README.md)。勾选表示本包实现和注明范围的验证/交付完成，不表示所有官方账号、所有模型或任意字面量复述均充分验收；未知继续明确列出，功能保持可用。原研究基线为 `fff38c01e771fed0523cfeb22d08baa88ae11b70`，发行代码为 `f85eca9d67801a970ba93312a553da602deacea3`。

## Global Constraints

- 不改 DSH、AG/HS 或其他客户端，不要求用户控制第三方后端；保留权限、归属、真实预算约束和具体不可表示请求校验；按2026-10-28用户裁决，缺少验收证据仅提示，不关闭可表达功能。
- 不重复开发已完成的 DeepSeek 角色/工具索引、Terra 用途编辑；先核查当日分支差异，保留无关工作。
- 无真实来源凭据时只做文档/形态/mock 验收，不能声称真实接入通过。问题单只作经验样本，优先遵守当前官方合同。
- 按用户授权批次在Mac提交；不同编辑文件可并行，重叠合同模块串行整合。测试前同步未提交修改及必要样本到隔离 build-test 目录，排除秘密；回收每轮日志和产物。
- 原生路径与转换路径分别标识；只修改受影响来源。已发首字节后不重试切源；不伪造思考、丢弃签名或重新计算不可证的用量。

## Review Focus

审查来源键是否过宽、默认值是否改变源语义、预算是否含思考/工具、签名归属是否覆盖新字段、usage 是否重复记账、规则升级是否误伤所有历史。每包评审只阻塞其受影响部分。

## 工作量与顺序

12 个核心包预计 **24–43 人日**，按熟悉项目的 Rust 工程师每日约 6 小时有效开发估算，包含针对性测试和审查，不含等待账号、供应方回复和发布窗口。不是工期承诺，首次 P0 完成后根据真实复杂度重估。

阶段 A 为 W01–W03，约 5.5–9.5 人日，先解决预算与归属风险；阶段 B 为 W04–W09，约 12–23 人日，逐来源补合同和回包；阶段 C 为 W10–W12，约 6–10 人日，诊断、真实验收及交付。W11 的样本工作随每包同步做，最终汇总时间勿重复计入；合计范围按各包原始值求和为 23.5–42.5 人日后取整。

文中短文件名均相对 `crates/server/src/`，`web ...` 相对 `crates/web/src/`；拟新增路径明确标记。实际修改前定位当前模块和测试位置，路径漂移时更新计划而非建立重复实现。

## W01 合同证据与版本边界

依赖：无。估算：2–3 人日。关联：G03, G19。

涉及文件：`crates/server/src/contract_evidence.rs（拟新增）；parameter_contracts.rs / protocol_config.rs / parameter_contracts_inventory.json`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 新增 contract evidence 纯数据结构；先记录当前规则，不批量改变行为。Ark 文档冲突单列未决，不因缺证关闭既有可表达功能。
- [x] 在 build-test 运行针对性检查：规则身份匹配及未知版本；旧配置兼容；文档刷新不改变会话代次；冲突不得自动提升为支持。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W02 输出上限与总预算

依赖：W01 的最小身份结构。估算：1.5–2.5 人日。关联：G01。

涉及文件：`parameter_contracts_china.rs / parameter_budgets.rs / gateway.rs / protocol/request.rs / request_plan.rs（拟新增）`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 由 W02 负责在现有 gateway 流水线增加轻量 RequestPlan 及字段处置结果；W01 只提供身份与证据。以不可变原请求生成副本，统一应用、最终校验、重新计量后接现有预占。区分总量、答案、未知；答案上限与思考预算组合算上界，先修百炼 Responses。
- [x] 在 build-test 运行针对性检查：Qwen3.8 与指定旧版本、min16、未知型号、thinking on/off、incomplete；预占不低于已证明上界；转换失败原请求不变，预算依据最终出站体，只预占/结算一次，原生路径不意外重写。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W03 签名与密文归属覆盖

依赖：W01；可与 W02 并行。估算：2–4 人日。关联：G02。

涉及文件：`protocol_sessions.rs / gateway.rs / protocol/sse.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 扩展原生 Chat 历史识别和递归遍历；签名与密文使用现有所有权机制，保留原形。
- [x] 在 build-test 运行针对性检查：Gemini extra_content 与 Ark encrypted_content 的 JSON/SSE、多轮、变造、错所有者、跨源、跨协议负例；旧签名形态回归。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W04 工具 strict 默认语义

依赖：W01。估算：1–2 人日。关联：G04。

涉及文件：`protocol/request.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 将工具 strict 作为三态；双向转换显式处理源语义，无法等价的 schema 保持拒绝。
- [x] 在 build-test 运行针对性检查：omitted/true/false × 两方向 × 可严格化/不可严格化 schema；原生直通不变，不能偷偷限制可选属性。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W05 国产来源角色合同

依赖：W01。估算：1.5–3 人日。关联：G05。

涉及文件：`message_roles.rs / gateway.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 复用 message_roles 的来源边界，为 Qwen/Kimi/GLM 精确模型添加官方角色规则，处理 system 例外。
- [x] 在 build-test 运行针对性检查：前导与中途 developer、纯文本/块/多模态、QwQ/QVQ 例外、失败原子性；OpenAI 与已验 DeepSeek 回归。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W06 厂商参数组合与型号更新

依赖：W01/W04；分来源独立提交。估算：4–7 人日。关联：G07, G08, G09, G11, G13, G14, G15。

涉及文件：`parameter_contracts_china.rs / parameter_contracts.rs / reasoning_capabilities.rs / protocol_config.rs / protocol/request.rs / protocol/sse.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 分别补 DeepSeek、Qwen、Kimi、GLM、Anthropic、MiniMax 规则；每项附证据，不做品牌级通配替换。
- [x] 在 build-test 运行针对性检查：每个 G07–G09/G11/G13–G15 的 allow/reject/default 配对样本；原生与转换分别验证；旧已验行为回归。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W07 响应元数据与使用量

依赖：W01；计量依赖 W02。估算：2–4 人日。关联：G10, G12。

涉及文件：`protocol/response.rs / protocol/sse.rs / upstream.rs / db/billing.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 区分 request_id 与语义扩展；定义缓存细分归一及 raw usage 留存，审计计费处是否实际受影响后再改结算。
- [x] 在 build-test 运行针对性检查：GLM request_id 可转换；未知语义扩展明确拒绝；Kimi 总输入子集守恒、缺项/终端重复 usage、幂等结算与流式等价。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W08 端点与运行时身份

依赖：W01。估算：2–4 人日。关联：G06, G17。

涉及文件：`parameter_contracts.rs / protocol_config.rs / upstream.rs / reasoning_capabilities.rs / web provider_keys.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 百炼 workspace 窄匹配加人工 profile；Ollama/vLLM 标准接口与操作者证据分级。
- [x] 在 build-test 运行针对性检查：正确/伪造域名，显式 profile 优先；同型号不同模板版本不混用；不可访问后端时仍可保存且显示未知。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W09 MiniMax 思考方言

依赖：W01/W06。估算：1.5–3 人日。关联：G16。

涉及文件：`protocol/request.rs / protocol/response.rs / protocol/sse.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 仅在明确来源启用 split/no-split codec；保留完整助手/工具历史；不使用全局正则。
- [x] 在 build-test 运行针对性检查：字符串与块、分帧标签、工具轮次、M3/M3.1 限制、错误形态与原生 passthrough；不制造不存在的历史思考。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W10 诊断与独立验证入口

依赖：W01；读取 W02–W09 结果。估算：2–3 人日。关联：G20。

涉及文件：`routes/admin.rs / routes/providers.rs / web pages/model_reasoning.rs / provider_keys.rs`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 增加版本化兼容诊断 DTO，复用现有管理端页面；保存与外部验证分离，展示证据层次。
- [x] 在 build-test 运行针对性检查：保存不发外部请求；探测限时与取消；权限/脱敏；用途、能力、可用性独立展示；目标视口浏览器检查。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W11 供应方与 SDK 验收矩阵

依赖：从 W01 起逐包累积，最终依赖 W02–W10。估算：3–5 人日。关联：G18。

涉及文件：`crates/server/tests/provider_compatibility.rs（拟新增）；tests/fixtures/provider_compatibility/（拟新增）；现有 SSE 测试模块`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 增加供应方形态 fixtures、mock 上游及受控实测脚本；issue 样本归档为回归输入，关闭状态不能当修复依据。
- [x] 在 build-test 运行针对性检查：空 tool_calls/重复 index/终端 usage/结束原因/中断；模型与鉴权来源精确记录；真实 JSON、SSE、工具多轮与错误验收，未实测明确标注。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## W12 分批发布与回滚验收

依赖：当批 W11 通过。估算：1–2 人日。关联：跨包交付门禁，不新增缺口数。

涉及文件：`CHANGELOG.md / docs/部署文档（实施时定位既有入口）；review/<当批验收>/`。

- [x] 对照关联台账的官方章节和代码，核实版本漂移及精确触发条件，写最小正反样本。
- [x] 整合已私有部署但未公开的修复记录；按安全补丁、合同补全、诊断分批交付，发布动作依当时授权。
- [x] 在 build-test 运行针对性检查：Mac 提交与待验哈希一致，build-test 日志取回；受控调用与独立公开包验收后整包切换，未充分验收状态不额外启停功能；回滚保护新会话状态；公开版本和生产版本分别确认。
- [x] 记录源码哈希、命令、退出码及结果并取回 Mac；样本、模拟和真实验证分层记载。
- [x] 审查差异并作独立本地提交，附启用条件和回滚方法；未充分验收项标注范围并保持可用；具体无权限或不可表示请求保留明确拒绝。

## 可选接入 O01–O09

以下只形成选项，不承诺与核心包同步完成；每项需要单独评估原生协议、工具、计量和凭据可用性后估算。依赖核心身份、预算和会话机制，不通过增加模型名称假装实现。

|包|台账|范围|实施前必须决定|
|---|---|---|---|
|O01|G21|xAI|可见输出上限、思考与工具预算及状态合同|
|O02|G22|Mistral 文本|ThinkChunk/TextChunk codec 与两种内容形态|
|O03|G23|Mistral connector|服务端工具与多 messages 执行器|
|O04|G24|Cohere V2|独立 /v2/chat 协议与 billed_units|
|O05|G25|OpenRouter|实际路由身份、require_parameters 和 reasoning_details|
|O06|G26|Gemini 原生|GenerateContent/Interactions 的独立状态和工具语义|
|O07|G27|Azure|部署名、认证线路、版本和地区|
|O08|G28|Google 托管/Vertex|认证、模型合同和兼容接口参数忽略行为|
|O09|G29|Bedrock|Converse 或兼容 API 的独立范围及工具执行模式|

每项交付须包含参数合同、原生/转换支持边界、最小工具多轮、JSON/SSE、usage 与预算、状态归属、错误与取消、真实账号验收记录。不具备执行器时保留明确拒绝。

## 执行与交付模板

每批在 Mac 记录 Git HEAD 与待验文件哈希，选择 `/srv/build/tokengo-<分支>-<批次>/`。先 `ssh build-test hostname` 确认主机；同步任务源码，排除 .env、私钥、登录缓存、真实数据库、Mac 依赖及二进制，正确反映删除。采用仓库当前锁文件安装 Linux 依赖，按实际改动运行 Rust 针对测试及必要 Web 检查；不用完整测试清单制造重复门禁。

失败也回传命令、日志、退出码、报告和生成产物到 Mac `review/provider-compatibility-<批次>/`；无产物明示。生产真实调用是额外验收层，遵守当时的授权和受控凭据方式，不把 build-test mock 通过当上游可用。

发布前审阅具体提交、版本说明、配置迁移、实测范围、待验证项及回滚包。依既有授权决定推送/发布；本研究文档交付本身不授权这些外部动作。既有私有部署修复须在CHANGELOG说明真实边界；方案文字本身不是完成证明，2026-10-28的交付结论以上述实际收据和明确未充分验收项为准。

## 定向检查入口（实施时在 build-test 执行）

以下是当前包名及模块入口，先同步当前源码再执行；预计退出码为 0，失败不得标记对应包完成。新增测试应先证明针对触发条件有效，禁止只断言实现常量。最终测试名随实际实现补入每批收据。

|包|最小检查命令/入口|必须额外覆盖|
|---|---|---|
|W01/W06/W08|`cargo test -p tokengo-server parameter_contracts`；`cargo test -p tokengo-server reasoning_capabilities`|精确来源、型号、协议组合与错误分支|
|W02|`cargo test -p tokengo-server parameter_budgets`|网关最终出站预算与预占/结算集成|
|W03|`cargo test -p tokengo-server protocol_sessions`|新签名路径的网关所有权负例|
|W04/W09|`cargo test -p tokengo-server protocol::request`；`cargo test -p tokengo-server protocol::response`；`cargo test -p tokengo-server protocol::sse`|默认值与流式分帧等价性|
|W05|`cargo test -p tokengo-server message_roles`|从网关入口到出站请求的正反例|
|W07|`cargo test -p tokengo-server protocol::response`；`cargo test -p tokengo-server billing`|缓存使用量守恒与重复终端事件|
|W10|`cargo test -p tokengo-web`；`cargo check -p tokengo-web --target wasm32-unknown-unknown`|既有 HTTP 管理接口与浏览器保存/独立验证行为|
|W11|`cargo test -p tokengo-server --test provider_compatibility`（拟新增）|供应方形态 mock、SDK 样本；真实来源脚本按批次另列|

过滤测试需检查实际运行数量，匹配 0 项不算通过。W12 按当批改动运行必要集成和回归；本表是计划入口，本次文档研究没有执行这些项目命令。

## 完成定义

- [ ] 20 项核心台账均有已实现/有依据保留限制/明确延期及原因，不能把未实现简单改成已完成。
- [ ] 每个开启能力有精确来源和四层证据状态，真实调用缺失项显式显示。
- [ ] 预算、归属、工具多轮、流式中断和计量守恒通过相应行为检查。
- [ ] 新旧会话与回滚路径可解释；未管理后端无需配合改造；客户端不变。
- [ ] 文档、代码和实际版本一致，日志产物回收，可按来源定位规则依据。

## 2026-10-27 第二批实施收据

W04、W05及W06的合同修复与保护性门禁已实施，细项、实际验收和未完成边界以[第二批收据](../../acceptance/2026-10-27-provider-p1.md)为准。MiniMax active Chat↔Messages 完整思考方言仍属W09；官方账号实测未由硅基流动结果替代。历史清单保留原计划，不将未完成的W07–W12勾选完成。

## 2026-10-27 第三批实施收据

W07–W09实现和验证范围见[第三批验收记录](../../acceptance/2026-10-27-provider-p1-batch3.md)。W09仅开放已证明的MiniMax API来源Chat split/Responses子集，签名Messages/no-split转换/未证实strict保证保留门禁；W08标准API解析所得仍是未验证声明，网络采集与完整诊断UI留W10。响应内raw usage留存不等于新增服务端账单审计仓库。
