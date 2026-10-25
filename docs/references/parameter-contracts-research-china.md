# 中国模型及本地服务：参数合同调查

2026-10-25。只读官方文档调查；没有修改源码、构建、运行测试、推送、部署或发起真实推理。基线 e7edfb3。机器可读明细见[matrix-china.json](matrix-china.json)。这是参数类别与已确认异常矩阵，不是“未来所有API嵌套字段已穷尽”的证明。

## 必须先纠正的通用假设

1. 关闭推理不是跨品牌统一枚举。Kimi K3、GLM5.3、MiniMax M3.1-Flash均有不可关闭模型；同品牌旧版可能能关闭。提供none必须绑定精确型号、来源与协议，不能从“reasoning=true”推导。
2. cap字段同名不等义。DashScope和SiliconFlow有answer-only上限；OpenAI式max_completion_tokens可包含推理。只有范围相同才是别名，不要以字段值相等就合并。
3. 原厂与转售平台合同不同。DashScope直供/第三方直供、SiliconFlow、Moonshot、BigModel、Ark不能因model字符串同品牌而共用参数表。
4. 原生参数接受不等于生效。部分API忽略温度/stop/penalties；应返回可解释的支持状态，而非宣称已适配，也不擅自改成服务商固定值。
5. native保留与cross-protocol转换分开。媒体、厂商工具、缓存、预测输出、部分生成、推理历史和加密状态应有逐字段决策；不能删除字段后把HTTP200当无损成功。

## 官方资料快照与重要差异

|来源|当前确认模型/合同|适配重点|
|---|---|---|
|[DeepSeek Chat](https://api-docs.deepseek.com/api/create-chat-completion/)、[思考指南](https://api-docs.deepseek.com/guides/thinking_mode/)|deepseek-flash、deepseek-v4-pro。当前Chat参考直接接受reasoning_effort=none，指南仍单列thinking开关；保留两种原生写法并显式检出冲突，不能硬断“none无效”。|max_tokens含总预算，stream_options须stream=true；工具强制选择与thinking冲突；末content chunk可带usage，不能只认choices=[]。|
|[DashScope Chat](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)、[Messages](https://help.aliyun.com/en/model-studio/anthropic-api-messages)|Qwen3.8/3.7/3.6等及各供应方模型，支持字段取决于精确来源。|max_tokens与max_completion_tokens不普遍等价；budget、图片像素、音频、搜索和代码解释器是各自语义。SDK的extra_body不是HTTP包装层。|
|[Kimi型号](https://platform.kimi.ai/docs/models)、[参数表](https://platform.kimi.ai/docs/api/models-overview)、[Chat](https://platform.kimi.ai/docs/api/chat)|K3.1 Pro/K3 always-think；K2.7-code只能保留思考；K2.6有关闭模式。K2.5文档已注明停用。|固定采样值不可改；历史推理、partial、prediction及cache读写需保留；缓存写入已计入输入总数。|
|[BigModel Chat](https://docs.bigmodel.cn/api-reference/模型-api/对话补全)、[Z.ai Chat](https://docs.z.ai/api-reference/llm/chat-completion)|GLM5.3只low/high/max；5.2有none/minimal关闭映射。|do_sample不是temperature0；tool_stream不等于stream；clear_thinking仅控制历史，不控制当前推理；MCP/检索不等于裸function。|
|[MiniMax OpenAI](https://platform.minimax.io/docs/api-reference/text-openai-api)|M3.1-Flash不可关闭；M3可disabled；M2.x的disabled可被忽略。|effort、reasoning_split、service_tier均型号相关；完整assistant/think内容回放，媒体字段保持原生。|
|[Ark型号](https://docs.volcengine.com/docs/82379/1330310?lang=en)、[迁移映射](https://docs.volcengine.com/docs/ark/model-deprecation-migration-guide?lang=en)、[深度思考](https://docs.volcengine.com/docs/ark/deep-thinking?lang=en)|Seed Evolving/2.1族；minimal在指定Seed模型表示关闭，不是OpenAI的轻推理。|加密原文与摘要不同；供应商stateful能力不等于TokenGo已允许跨用户续接。Managed Agents参数表不能代替普通HTTP schema。|
|[SiliconFlow Chat](https://docs.siliconflow.cn/docs/api/chat-completions-post)|示例为DeepSeek-V4-Flash及Kimi-K2.7-Code，按平台实际目录选择。|max_tokens文档明确不含CoT，thinking_budget独立；enable_thinking、reasoning_effort和min_p各有型号条件，不能套原厂规则。|
|[Ollama兼容](https://docs.ollama.com/api/openai-compatibility)|模型由实例目录决定；/api/show提供推理元数据。|OpenAI子集；Responses无stateful；服务商会对部分不支持effort回退默认，网关不能把成功当参数生效。|
|[vLLM兼容](https://docs.vllm.ai/en/stable/serving/online_serving/openai_compatible_server/)|模型、模板、解析器与generation_config决定实际合同。|保留采样/模板/structured_outputs扩展；版本不同parallel_tool_calls行为不同；cache_salt不是身份授权。|

## 对 TokenGo 匹配器的具体建议

使用“提供方 + 精确模型/版本 + 协议 + 当前目录/配置身份”选择合同；不能将支持表按厂商品牌静态展开为所有候选模型。至少区分下面四类结论：

- native_preserve：按原生字段完整保留，上游实际接受/忽略/拒绝仍按精确合同报告。
- lossless_map：只有名称、范围、默认值、计量范围及状态含义均一致才转换，保留显式null/false/0与省略的区别。
- model/source adapter：例如平台enable_thinking、thinking、named effort，明确来源/型号适用条件，不能全局猜。
- reject conversion：当前目标协议不能表达的媒体、工具、缓存、预测、opaque/state字段，明确指出字段和理由；不采用drop_params式静默成功。

安全/账务边界不可被“参数适配”放松：用户模型权限、Key与会话归属、供应商账务原值、cap与TPM预占语义、token/cache去重计量均需独立验证。此调查只确认文档合同，没有真实供应商账号验收。

## 未穷尽与待核实

矩阵覆盖常用生成端点及典型参数族，未枚举所有未来schema、所有嵌套media/tool字段或所有区域直供组合。DashScope/Ark的Responses与Messages原生合同需要按端点进一步逐项引入；本轮不能从managed-agent说明推出普通API支持。文档中存在官方页面不一致，已记录，优先保留原生payload和来源版本，不能擅自删参数来消除差异。所有来源页面查阅日期均为2026-10-25。

DeepSeek双开关补充：当前Chat参考未查到thinking与reasoning_effort同时冲突的优先级定义。网关若为防歧义拒绝，应说明这是本地策略，不得写成上游必然返回400的实测结论。

## Xiaomi MiMo 补充（2026-10-25）

官方三协议当前列出 `mimo-v2.6-flash`、`mimo-v2.6-pro`、`mimo-v2.6-pro-ultraspeed`。来源 profile 为 `mimo`；Chat/Responses base 为 `https://api.xiaomimimo.com/v1`，Messages 为 `/anthropic/v1`。不能从 MiMo 模型名推断托管商使用相同合同。

Chat/Messages 使用 `thinking.type=enabled|disabled`，默认 enabled。Responses 的 `reasoning.effort=none` 关闭推理，其他有效档位目前都开启相同推理行为。仅关闭语义可无损映射，不能宣传不同档位已具有不同推理强度。采样 temperature 0..1.5、top_p .01..1；推理模式会强制 1/.95，保留字段不等于字段有效。[Chat](https://mimo.mi.com/docs/en-US/api/chat/openai-api)、[Messages](https://mimo.mi.com/docs/en-US/api/chat/anthropic-api)、[采样](https://mimo.mi.com/docs/en-US/api/guidance/model-hyperparameters)

Chat `max_completion_tokens` 与 Responses `max_output_tokens` 明确包括推理，范围1..131072；Messages `max_tokens` 为绝对生成最大值，相同范围。Responses 不支持 background、previous_response_id、context_management。完整历史 reasoning/tool blocks 和缓存/多模态 usage 必须保留，不能把 summary 当原文。[Responses](https://mimo.mi.com/docs/en-US/api/chat/responses)

官方只承诺 auto 工具选择；非 auto 会在上游丢弃并退回 auto，产品应明确拒绝此语义损失。输出格式目前只列 text/json_object，不能把工具的 strict schema 误作支持输出 json_schema。当前源码增加精确型号校验与 none 关闭适配，无上游实测证据，不自动构建。

### SiliconFlow Messages 输出计量补证

[官方 Messages](https://docs.siliconflow.cn/docs/api/messages-post) 也明确 max_tokens 排除 CoT，并提供 thinking.type 与 budget_tokens；不能套用 Anthropic 通用总输出 cap 语义。无 cap 的首次 admission 仍需保守判断 answer-only。未知模型即使带 budget 也不把未经证实有效的值当硬限制。
