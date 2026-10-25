# LiteLLM 参数适配源码与 TokenGo 缺口审计

2026-10-25。状态：只读静态源码审计，未构建、未测试、未导入下载的 Python、未修改项目源码、未推送或部署。TokenGo 基线 `e7edfb3dbceb566f1bf9f00d02a7f9adb1cca873`，LiteLLM 固定 `72f1ac4a2795e160d1044fb963041aec3111ff70`，不以可变化的 main 分支内容充当固定证据。14 个所查 LiteLLM 文件及其哈希见 `litellm-source/manifest.json`；全部在 enterprise 外，根许可证声明这部分为 MIT（Copyright 2023 Berri AI）。若后续实质借用代码，应保留相应许可归属；本报告不推荐引入 Python 运行时或企业代码。

## 借鉴的架构

LiteLLM把“支持参数清单、值映射、请求变换、响应变换”拆在 provider config 中，模型注册数据和 deployment 声明再约束条件。OpenAI Chat/Responses、Anthropic、Gemini与DeepSeek有各自转换器；参数清单是该层的入口，不是所有模型、账号和传输都支持的保证。其 Responses 接口从类型定义取得字段集，再执行具体模型校验。可借鉴这种分层，以及支持矩阵和互斥条件与实际变换共用同一份描述。证据：[base](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/llms/base_llm/chat/transformation.py)、[openai](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/llms/openai/chat/gpt_transformation.py)、[responses](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/llms/openai/responses/transformation.py)。

## 不应照搬的行为

- drop_params/allowed_openai_params会删除不支持字段或绕过支持过滤。这与完整保留客户端意图不同，不能成为 TokenGo 默认行为。应返回带字段路径、目标协议和原因的明确错误；原生未知扩展保持透传但标为未验证。
- 有些适配会修改历史、默认指令或输出机制。DeepSeek helper在缺少推理历史时可以补单空格；Anthropic可用强制工具模拟JSON输出、调整thinking预算或把adaptive改成legacy预算。它们解决的是兼容目标，不是无损等价。TokenGo不应伪造 reasoning/signature、秘密增加工具、削弱JSON Schema或改变用户预算。
- 私有 ChatGPT adapter 的endpoint是 `https://chatgpt.com/backend-api/codex`，并注入指令/强制include/过滤请求键。它不能证明 TokenGo 独立 public OAuth/SIWC 在 `api.openai.com/v1` 上的参数能力，尤其不能据其过滤表删除输出上限或格式控制。

证据：[drop](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/utils.py)、[deepseek](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/llms/deepseek/chat/transformation.py)、[anthropic](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/llms/anthropic/chat/transformation.py)、[chatgpt](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/llms/chatgpt/responses/transformation.py)、[chatgpt_endpoint](https://github.com/BerriAI/litellm/blob/72f1ac4a2795e160d1044fb963041aec3111ff70/litellm/llms/chatgpt/common_utils.py)。

## 当前 TokenGo 的实质结论

`source-audit.json`逐项给出12条发现、源文件行号、证据等级和建议。最直接的增量是对全部参数建立逐项处置，而不是把跨协议白名单不断扩展成无来源约束的万能字段表。

1. Native convert早退clone，但gateway仍执行成本/媒体/状态守卫、显式drop_fields、来源默认和OAuth preparation。不能把“native clone”宣传成任意字段均无条件支持。
2. 当前跨协议白名单拒绝多数较新的控制字段。Responses `text.format`反向到Chat、verbosity与部分日志概率字段缺乏对应映射；其他如seed、penalties、logit_bias未必在目标API有等价物，应在矩阵中明确拒绝而非编造映射。
3. sampling仅有通用数值范围检查，source/model/transport级互斥与effort条件需要独立合同。已有reasoning层是可借鉴的身份绑定模式，不应把价格表当能力事实。
4. 配置drop_fields会在转换后删除语义控制；learned_output_cap会降低待发送cap。它们必须与参数匹配分开成为明确策略。后者存在掩盖原始超界请求的派生风险，应先保留并校验用户原cap，再决定是否允许收窄；本审计没有执行请求验证该风险。
5. Messages的service_tier转换已经明确拒绝，不能把它误报为当前静默透传漏洞。Chat/Responses之间仍需按来源核对tier枚举和实际可用性。
6. 工具、strict、JSON格式、完整历史、缓存与多模态需要各自合同。不能把server tools/托管检索/容器当函数定义改名，也不能仅新增一个参数名就绕过计费和opaque会话边界。

## 可实施的分层与文件边界

建议新增 `crates/server/src/parameter_contracts.rs` 作为纯匹配/诊断入口，`parameter_contracts/profiles.rs` 存有官方来源、传输与精确型号证据的规则。若还需代理/自建端点能力配置，应使用管理员明确选择且与来源身份绑定的有界配置，不从模型名猜平台。供应商/模型支持数据、协议字段schema、参数变换应彼此独立。

建议请求处置模型包含：字段JSON pointer、值类型/范围、条件谓词、provenance、处置(pass_native / rename_lossless / provider_translate / reject / unknown_native)、目标路径、固定安全原因。生成 adaptation plan 后一次性应用；记录 consumed/produced 字段集合和正文哈希，不把用户输入、metadata值或凭据写入诊断日志。每个字段必须有处置，无法保证无损的变换不得默默采用近似值。

`protocol/request.rs`保留独立的消息/工具/内容变换，`reasoning_capabilities.rs`保留有效目录/人工列表/默认值优先级，`protocol/collect.rs`保留OAuth特定传输。`gateway.rs`只负责按真实来源选择、执行计划和既有权限/计费/重试/会话门禁；先匹配用户原始意图，再处理别名/默认值/optional策略，最后对实际发出的请求重新计量。matcher不能扩大候选、绕过TPM预占或降低已验证的上下界。

配置与管理DTO应能报告“字段被哪个来源规则允许/转换/拒绝”，而不暴露source fingerprint的密钥材料。管理员手工确认必须仍优先，来源凭据/endpoint/protocol/generation变化应使证据失效。现有 `protocol_config.rs`与providers reasoning配置入口是集成边界；不要把全参数规则混进已定价字段或把私有DB种子导入新用户。

## 必须编写但本轮不自动执行的检查

逐协议对同一参数覆盖省略/null/false/0、错误类型、未知枚举、边界/越界、冲突别名、默认与客户端显式值、unknown/negative capability、来源身份变更、manual排除、原生扩展保留、dry-run不改原body、计划字段守恒。再覆盖JSON Schema不改写、严格工具不降级、reasoning/opaque原文不伪造、SSE提交后不重放、模型所有权与source计费边界不变。

只有后续用户允许验证时，才按原有build-test流程同步精确源码执行。此处未运行测试，也不以静态审查代称通过。

## 覆盖边界

本审计查了13个适配/类型/工具源文件与1个许可证，未逐字审阅LiteLLM全部供应商和所有测试。厂商当前参数语义应由并行的官方文档矩阵补充。LiteLLM注释可能滞后，所以其值域/预算/模型名单只能作为比较材料，不能直接升级为TokenGo自动能力事实。

## 已实施的安全协议增量（未编译、未执行）

按主代理分工，仅改现工作树protocol/request.rs与protocol/mod.rs。新增Chat/Responses双向text/json_object/json_schema格式匹配，保留schema/name/description/strict与strict:null；verbosity与text.verbosity对应，保留显式null。user、safety_identifier、prompt_cache_key、prompt_cache_options与prompt_cache_retention同名语义保留；cache options仅共同mode/ttl结构，Responses独有comparison_response_id/prewarm反向Chat明确拒绝；retention与ttl从不相互改名。Messages不具备这些对应时明确拒绝，原生同协议clone保持不变。

已编写7项单元检查，覆盖双向roundtrip、null/false/省略区分、并存格式与verbosity、冲突/错误alias形状、unknown nested字段、缓存值域、全部6方向的明确处置。原先cache-key被删的legacy单测改为断言保留，但规范化诊断仍不含用户值。仅执行git diff --check，没有编译或执行测试；不能称用例已通过。

TTL当前官方资料存在差异：REST reference将request TTL列为30m，官方prompt-caching guide与最新版LiteLLM类型又包含12h。语法层保留这两个已出现的原值，实际源/模型/账号支持由来源合同另行约束；没有把24h retention近似为ttl，也没有据此宣称所有模型可用12h。

新请求格式映射不代表新实现了响应侧的refusal/annotations/logprobs等完整转换。response.rs依赖原decode_messages、SSE转换器也有特殊块拒绝路径；必须保持明确原生/拒绝边界，不能将拒答伪装成普通JSON或悄悄删除概率、注解数据。若要扩展它们，需要同步canonical响应表示、buffered、streaming和回放，不宜只修一条分支。

官方参数依据：[Chat创建](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、[Responses创建](https://developers.openai.com/api/reference/resources/responses/methods/create)、[缓存指南](https://developers.openai.com/api/docs/guides/prompt-caching)。
