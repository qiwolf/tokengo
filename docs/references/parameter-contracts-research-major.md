# OpenAI、Claude、Gemini 参数合同调查

日期：2026-10-25（Asia/Shanghai）。状态：官方文档与固定官方SDK源码静态研究；**没有构建、测试、真实推理或生产写入**。Tokengo基线为`e7edfb3`。机器清单为`matrix-major.json`：逐项列出127个顶层参数/配置项，包含来源URL和SDK SHA、类别、原生/转换处理原则、当前转换白名单是否覆盖。它不是127项均已适配的声明，也不是所有嵌套工具种类的穷尽证明。

## 证据固定

官方SDK仅下载研究、AST读取，不导入、不执行：

|SDK|固定提交|读取对象|
|---|---|---|
|[OpenAI Python](https://github.com/openai/openai-python/tree/718aee979a9c334d37302aeff21cee6cbff4d43c)|`718aee979a9c334d37302aeff21cee6cbff4d43c`|Chat 36字段+stream；Responses 31字段+stream|
|[Anthropic Python](https://github.com/anthropics/anthropic-sdk-python/tree/202db72c87d7c6eaa72129c67ce67fd343a3a20c)|`202db72c87d7c6eaa72129c67ce67fd343a3a20c`|Messages 18字段+stream|
|[Google GenAI Python](https://github.com/googleapis/python-genai/tree/cb1408b06c573060f1593023ca320fde9a4fb5f6)|`cb1408b06c573060f1593023ca320fde9a4fb5f6`|GenerateContentConfig 39配置项，另读ThinkingConfig等|

原文件和提取结果在`evidence/official-major/`，索引为`source-index.json`。Google SDK配置不等于REST原生JSON字段；`http_options`、`automatic_function_calling`等属于SDK执行控制，不能直接塞进Chat正文。

## 最新常用型号与关键组合

### OpenAI

当前官方推荐`gpt-6-astra`、`gpt-6.1-sol`和`gpt-6-luna`。Astra和Sol6.1的effort为low/medium/high/xhigh/max，不能使用none/minimal；Luna另有none。三者公开上下文1050000、最大输出128000；这不是任何OAuth账号的自动额度配置。[型号目录](https://developers.openai.com/api/docs/models)

GPT6的工具和采样约束由型号及**生效effort**共同决定：Astra、Sol6.1 Chat不能工具调用；Sol6/Luna仅在none下支持Chat函数调用。非none时sampling/logprobs字段不适用，应明确错误或让调用方选择正确参数，不能自动降档/删字段。[模型迁移合同](https://developers.openai.com/api/docs/guides/latest-model)

新缓存`prompt_cache_options`与旧retention不是机械别名。Chat的TTL在SDK/guide与其他REST资料间存在差异，不能据其中一份资料硬拒12h；此项标条件性，保留原值由当前来源处理。mode和内容断点影响命中及写入收费。`prompt_cache_key`不是会话/授权身份。[缓存文档](https://developers.openai.com/api/docs/guides/prompt-caching)、[Chat参考](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)、[Responses参考](https://developers.openai.com/api/reference/resources/responses/methods/create)

### Claude

当前型号为`claude-fable-5-1`、`claude-opus-5-5`、`claude-sonnet-5-5`、`claude-haiku-5-5`。[型号目录](https://platform.claude.com/docs/en/models/overview)

Thinking与effort独立。Fable5.1/Opus5.5不能关闭；Sonnet5.5只允许high及以下的between_tools，仍不是完全关闭；Haiku5.5在high及以下可disabled。adaptive-only模型不能强塞enabled预算。Models API逐值报告effort及thinking.types能力，应优先读取真实模型元数据。[Thinking合同](https://platform.claude.com/docs/en/build-with-claude/thinking)、[Models API](https://platform.claude.com/docs/en/api/http/models/retrieve)

API版本是独立匹配轴：`anthropic-version>=2026-10-21`移除temperature/top_p/top_k，任何显式值均报错。旧版本仍有型号/模式限制，不能把新版本规则套到未知第三方或旧header。[版本合同](https://platform.claude.com/docs/en/api/versioning)、[迁移说明](https://platform.claude.com/docs/en/api/migration-2026-10-21)

最新Messages支持`max_tokens=0`缓存预热；需保留输入用量/收费，不能转换成普通正数生成或普遍松开其他来源下限。effort、`output_config.format`、工具严格模式不是同一个功能，不可用伪造工具请求替代原生JSON Schema。[Messages参考](https://platform.claude.com/docs/en/api/messages/create)、[结构化输出](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)

### Gemini

当前常用`gemini-4-argon`和`gemini-3.8-flash`，原生thinking level为low/medium/high。Argon默认同步输出65536，long decode另有上界；只有隐式缓存。3.8原生minimal不适用。[模型目录](https://ai.google.dev/gemini-api/docs/models)、[Argon](https://ai.google.dev/gemini-api/docs/models/gemini-4-argon)、[3.8 Flash](https://ai.google.dev/gemini-api/docs/models/gemini-3.8-flash)

OpenAI兼容端点与原生GenerateContent不是同一合同。兼容页把Argon的reasoning_effort=minimal映射到原生low，不能拿原生枚举直接拒绝别名；3.8的兼容minimal映射未在该表单独确认，应保持未知而不猜。reasoning_effort与extra_body.google.thinking_config的level/budget互斥。2.5 Pro及3系列不能普遍加none；2.5 Flash/Flash-Lite兼容层允许关闭。[兼容层](https://ai.google.dev/gemini-api/docs/openai)

Google专用cached_content、thinking_config、安全设置、grounding、媒体等须保留其字段层级和端点；现Tokengo三协议不是原生GenerateContent transport。未知兼容字段不应本地自动删除，也不能据能透传就宣称已支持Gemini全部原生能力。[GenerateContent参考](https://ai.google.dev/api/generate-content)

## 全参数处理类别

|类别|参数例|处理原则|
|---|---|---|
|输入/角色|messages/input/instructions/system、content blocks|文本及客户端工具子集可明确转换；developer/system差异、图像音频/PDF/引用、复杂顺序需来源合同；不能把所有内容压成字符串|
|输出上限|三个max字段、n、max_tool_calls、truncation|硬上限与推理开销、联合窗口、账号上界分别校验；别名冲突拒绝；不把采样预算/工具次数视为输出cap|
|采样/概率|temperature/top_p/top_k、penalties、seed、logit_bias、logprobs|类型与型号/版本组合检查；没有同义目标即原生或拒绝，不近似模拟|
|推理|effort、thinking.type/budget/display、summary/context/mode|档位、开关、显示、状态互不等价；不造预算/签名，不自动把none改low或between_tools|
|工具|tools/tool_choice/parallel、hosted/MCP/custom/versioned tools|客户端函数仅明确子集；服务端工具的权限、计量、状态与schema必须原生保留|
|结构化|response_format、text.format/verbosity、output_config.format|支持确切schema层级和strict；不能删schema约束来求200|
|流式|stream、stream_options、事件序号、finish/stop原因|正文转换之外，还要终态、取消、截断、用量和错误语义；不合并/重复计费|
|缓存/会话|cache_control/options/key、store、conversation、previous_response_id、container|缓存与身份分离；状态需同源同账号归属；后台请求需异步生命周期与最终账务，不能仅透传字段|
|安全/位置/服务等级|moderation/safety_identifier、safety_settings、service_tier、speed、inference_geo|名称相似不保证枚举、收费或地域语义相同；保留用户意图，源合同不清时拒绝跨协议|
|多模态/生成|audio/modalities/prediction、image/video/speech config|不同transport/形状不能伪装文本兼容；区分SDK控制项与wire字段|

## 不透明状态和账务

OpenAI encrypted_content、Claude signature/redacted_thinking和Google thought_signature都应原样、按顺序且绑定实际来源回放。不要剥离signature“修好”错误，不把不同供应商的opaque字段混作一种思考文本。[OpenAI Responses](https://developers.openai.com/api/reference/resources/responses/methods/create)、[Claude thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)、[Google signatures](https://ai.google.dev/gemini-api/docs/thought-signatures)

输入、可见输出、推理、缓存读写及各TTL计数须以实际源用量结算；缓存写入不是已缓存读取，子计数不要重复加总。native clone只证明wire字段没被适配器删除，不证明后台状态、hosted工具、媒体计量、取消或跨协议执行器已经完整实现。

## 对当前代码的直接影响

`protocol/request.rs::convert_request`同协议clone，跨协议白名单明显小于当前官方字段面。建议先建立声明式逐字段库存和来源/型号/API版本/协议规则，再分别补可证明的Chat↔Responses映射。多媒体、异步会话、托管工具及安全参数保持原生或明确拒绝；不要选择LiteLLM的drop_params式宽松降级。

本报告不构成运行验收。按用户本轮约束，新增代码、未来回归用例可以准备，但不自动编译、构建、测试、推送或部署。

## 追加：内容状态不能按角色粗判

Claude4.6+拒绝不完整assistant预填，但官方pause_turn恢复示例会把来源assistant内容原样作为末项重发。不能一律拒绝最后assistant，也不能将server_tool等待态当普通end_turn；需保留源内容、工具定义和状态归属。[stop reasons](https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons)

## 审查修正

公开cache retention最大策略与新TTL最小生命周期不是等价别名，也不意味着两字段互斥。早期新模块曾从“不等价”过度推断为“共存拒绝”，root静态审查已指出并负责移除；原生两个字段需保留，不自动删改。Chat 12h TTL资料差异按未决/条件性处理，不以文档冲突缩小可接受原生请求。这个修正仅是静态研究与源代码审查，仍未执行构建或测试。
