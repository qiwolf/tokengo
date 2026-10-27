# 模型提供方接口资料册（2026-10-27）

本册覆盖20类来源，整理官方参数、协议特点、兼容注意事项与当前源码边界，供后续设计引用。源码只读基线为 `fff38c01e771fed0523cfeb22d08baa88ae11b70`。本轮没有改动实现、生产或客户端，没有运行模型调用或构建测试。资料收集完成、代码已存在、运行实测通过是三种不同状态。

来源逐条见 [sources.json](sources.json)，issue/PR状态见 [issues.json](issues.json)，统计与范围见 [collection-summary.json](collection-summary.json)。ID采用 `group:原ID`，同URL重复核验仍保留记录，并另行统计唯一URL。旧10-25未重新核实结论不作为本册事实。

设计结论见[方案书](../../superpowers/specs/2026-10-27-provider-compatibility.md)，实施顺序见[开发计划](../../superpowers/plans/2026-10-27-provider-compatibility.md)，逐项工作见[gap-ledger.json](gap-ledger.json)。

实施进度：本资料册及JSON台账保留研究时刻的基线状态，后续结果单独见[第一批P0验收记录](../../acceptance/2026-10-27-provider-p0.md)及[第二批 W04–W06 验收记录](../../acceptance/2026-10-27-provider-p1.md)，不将未完成的P1/P2合并宣称通过。

## 阅读与参数核验口径

| 维度 | 本册统一口径 |
|---|---|
| 角色 | developer/system不是可任意互换的文字标签；模型角色枚举和系统提示时序分别核验。 |
| 推理/历史 | effort、开关、budget、summary、原始思考、加密签名不同；省略值也是语义。工具续接保留上游完整结构，不能伪造空思考或以摘要替代签名。 |
| 工具/schema | client function、custom、namespace、server/hosted tools分开；required、named、parallel、strict与JSON格式按来源/型号/协议核验。 |
| 采样/输出上限 | 字段被接受不等于生效；context窗口、可见答案上限、总生成上限、thinking_budget分别记录。SDK extra_body不等于HTTP需要额外套壳。 |
| 缓存/状态 | cache read/write、store、previous_response_id、后台任务和资源CRUD是不同能力；来源归属不能只依赖上游坏签名报错。 |
| 媒体 | 输入图片、音频、文件、视频与输出媒体需专有能力和计量；文本转换成功不证明媒体支持。 |
| 流/usage/error | HTTP200、ping、收到delta不等于完成。区分终止事件、早EOF、累计usage和独立尾帧；不能重复累计或编造零成本。 |
| 鉴权/目录 | APIKey、OAuth计划、云部署、运行时、聚合器分别标来源。目录可见不证明账户可推理；未知能力保持未验证。 |

read表示已读取列明章节，不表示全文/所有型号。partial是索引、截断或仅部分正文，failed是未取得正文；后两者不计完整已核验文档。未单独收集的字段仍未知，不能由OpenAI形状补齐。

## 20类来源

### 1. OpenAI（公共API；OAuth另列子线路）

**范围：** 公共 Chat/Responses；手工历史规则只按明确模型家族；ChatGPT计划 OAuth/Codex app-server 独立授权。

**主要参数与特点：** developer/system保持语义；reasoning.context/effort和phase依型号；完整output原序续接。工具支持required/parallel/strict，但Chat省略strict默认非严格，Responses尝试严格化并可回退；custom/hosted不能当普通function。schema严格模式有子集约束。

**当前代码状态：** 公共API/OAuth合同分离；native clone保留字段、转换严格拒绝不可表示状态；tools转换省略strict尚有语义差异。目录可列出不代表OAuth账户有权限。

**注意事项与未知边界：** 不由模型名/打包目录证明可推理；新phase/context跨协议映射、精确model采样/cap及工具费用需另证。

**官方来源：** [major:OA01 · Reasoning models](https://developers.openai.com/api/docs/guides/reasoning)（read）；[major:OA02 · Function calling](https://developers.openai.com/api/docs/guides/function-calling)（read）；[major:OA03 · Codex app-server token sharing](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)（read）。

### 2. Anthropic

**范围：** 原生 Messages与官方Chat兼容分线；2026-10-21 APIversion；精确Opus/Sonnet/Haiku/Fable/Mythos型号。

**主要参数与特点：** adaptive与legacy budget不同；签名/redacted（脱敏思考）块完整回放。APIversion删除sampling；旧版本也有型号固定采样。disabled/between_tools/forcedtools依型号。max_tokens=0为专门缓存预热，有stream/thinking/format/forcedtools互斥。流usage累计、HTTP200后仍可能流内报错。

**当前代码状态：** 已有Messages版本/型号合同与预热门禁；Mythos5.1等未进入当前精确表。官方Chat会合并system/developer并忽略strict/format/effort，不能等同原生。

**注意事项与未知边界：** 新型号枚举、账户绑定签名和跨模型读取范围不能泛化；完整结构化/服务端工具及媒体依原生能力。

**官方来源：** [major:AN01 · Claude thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)（read）；[major:AN02 · Claude streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)（read）；[major:AN03 · Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)（read）；[major:AN04 · OpenAI SDK compatibility](https://platform.claude.com/docs/en/cli-sdks-libraries/libraries/openai-sdk)（read）。

### 3. DeepSeek

**范围：** official Chat deepseek-flash/deepseek-v4-pro；official stateless Responses

**主要参数与特点：** roles：Chat四角色；Responses developer按user。已有TokenGo Chat前置文本角色适配。 thinking：默认开；none关闭；当前参考low/high/max及兼容别名。思考与工具历史需原样保留。 tools：Chat函数严格模式有beta边界；思考模式不支持required/named。Responses custom仅apply_patch、server web_search仅特定型号，其余有忽略行为。 structured_output：Chat text/json_object；Responses另有json_schema，不应强行视为Chat同能力。 sampling：thinking下temperature/presence/frequency无效；top_p实际行为随型号不同。 caps：Chat max_tokens含思考和答案；Responses max_output_tokens含总生成。 cache_state：缓存自动管理；Responses store/previous_response_id等非有状态执行能力。 stream_usage：Chat DONE、末choice带usage；Responses completed/incomplete/failed，无DONE；包含reasoning_tokens。 media：原生图像有型号/角色限制；现跨协议图像拒绝保留，不能静默转system或抹去。

**当前代码状态：** 已有文本developer前置适配、非流tool_calls.index定向校验和思考续接修复；旧验收收据不等于本轮新实测。原生Responses子集合同仍需补。

**注意事项与未知边界：** ultra等别名需按实际协议/当前参考再核实，不能从其他托管商借枚举；原生Responses能力与现已验收转换路径应分别做受控验收。

**官方来源：** [china:CN-S01 · Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion/)（read）；[china:CN-S02 · Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/)（read）；[china:CN-S03 · Using Responses API](https://api-docs.deepseek.com/guides/responses_api/)（read）。

### 4. 阿里百炼 / DashScope

**范围：** Qwen3.8与较早Qwen；百炼自营及第三方直供型号；Chat/Responses独立

**主要参数与特点：** roles：文档消息变体无developer；QwQ/QVQ对system另有限制。 thinking：Qwen3.8 none/low/medium/xhigh及官方映射；older与托管GLM/Kimi有独立开关和强度，preserve_thinking模型限定。 tools：Qwen required与thinking named存在限制；parallel=false对部分托管来源不构成串行保证。 structured_output：Chat可json_object/json_schema，需精确型号；Responses含原生工具与状态，不应泛化至Chat。 sampling：温度与top_p按型号约束；SDK extra_body不是HTTP额外套壳。 caps：max_tokens/ max_completion_tokens作用范围不同；Responses Qwen3.8总输出、列明older型号仅答案；最小16。 cache_state：输入缓存有细分；Responses previous_response_id等需要专门生命周期与归属门禁。 stream_usage：Chat空choices usage与DONE；Responses completed/incomplete，托管工具有额外事件。 media：Qwen音频/视觉/文件输入有协议与像素上限差别，不因为同厂商就保证跨协议。

**当前代码状态：** 已有部分精确型号合同/AnswerOnly预算；不能据厂商名涵盖托管模型、地区及新workspace端点。

**注意事项与未知边界：** 新workspace主机需安全身份/显式profile，不任意后缀推断；第三方直供ID、地区和账号开通是实际接入前提。

**官方来源：** [china:CN-S04 · OpenAI compatible Chat](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)（read）；[china:CN-S05 · Create a response](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses)（read）；[china:CN-S06 · Deep thinking models](https://help.aliyun.com/en/model-studio/deep-thinking)（read）。

### 5. Moonshot / Kimi

**范围：** K3.1/K3；K2.7 Code/Highspeed；K2.6；Chat/Responses独立

**主要参数与特点：** roles：Chat原始OpenAPI四角色，不含developer。 thinking：K3常开+Preserved，effort low/high/max；K2Code enabled keep=all，K2.6可关闭，原历史不改。 tools：K3 Chat支持required、K2不支持；Responses schema仅auto；function/custom apply_patch/namespace/server search需区分执行器。 structured_output：Chat原始MD明确json_schema；Responses也有结构化text schema，不能由网页折叠搜索缺失判断不支持。 sampling：文档固定temperature/top_p/n/penalty规则，现Kimi Chat校验已覆盖已列确切型号。 caps：新接口优先max_completion_tokens；K3文档给默认与上限、输入余量可能使上游降低生成上限；不能当上下文总长。 cache_state：5m/1h implicit cache；cache读/写/未缓存是input内互斥桶；原生Responses无持久状态。 stream_usage：Chat reasoning后content、空choices usage；Responses有reasoning_summary族事件及失败/不完整终止。 media：原生视觉内容应按模型目录；跨协议图片仍不保证。

**当前代码状态：** 已有Kimi精确型号采样/thinking合同；Chat与原生Responses、缓存写入计量/别名版本仍不应合并。

**注意事项与未知边界：** 账号订阅Kimi Code目录权益不同于公共API；公共文档不能为OAuth账户授予型号；无真实凭据，本轮只核实文档/代码，不记实测。

**官方来源：** [china:CN-S07 · Chat Completions API](https://platform.kimi.ai/docs/api/chat)（read）；[china:CN-S08 · Model Parameter Reference](https://platform.kimi.ai/docs/api/models-overview)（read）；[china:CN-S09 · Responses API](https://platform.kimi.ai/docs/api/responses)（read）；[china:CN-S10 · Thinking Models](https://platform.kimi.ai/docs/guide/use-thinking-models)（read）。

### 6. 智谱 / Z.ai

**范围：** GLM5.3/5.2等公共API；Coding Plan为不同产品合同；Chat

**主要参数与特点：** roles：当前schema四角色；不含developer。 thinking：5.3 API不可关闭，只low/high/max；Coding Plan aliases映射另表；clear_thinking控制跨turn历史，不改变当轮输出。 tools：schema tool_choice仅auto；tool_stream必须配stream=true；server search/MCP另有语义。 structured_output：原始OpenAPI enum仅text/json_object；描述“三种”不等于json_schema已支持，保留冲突待确认。 sampling：do_sample=false时sampling参数被忽略；temperature/top_p范围与stop数量有明确限制。 caps：max_tokens为输出限制，系列上限不同；与context窗口不是同一字段。 cache_state：有cached_tokens；成本是否按缓存计价需按来源价格/用户合同分别验证。 stream_usage：Chat DONE；delta可含reasoning与tool_calls；响应顶层request_id需类型化保留。 media：原生audio/video/search/content_filter有额外语义；不能只为了转换成功全部丢弃。

**当前代码状态：** 已有部分型号effort门禁；Chat角色/response元数据/新schema边界仍有缺口。

**注意事项与未知边界：** Flash系列确切可用ID先由目录核实，不能按品牌缩写猜；本轮未做真实GLM请求。

**官方来源：** [china:CN-S11 · 对话补全](https://docs.bigmodel.cn/api-reference/模型-api/对话补全)（read）；[china:CN-S12 · 深度思考](https://docs.bigmodel.cn/cn/guide/capabilities/thinking)（read）；[china:CN-S13 · 工具流式输出](https://docs.bigmodel.cn/cn/guide/capabilities/stream-tool)（read）；[china:CN-S14 · 核心参数](https://docs.bigmodel.cn/cn/guide/start/concept-param)（read）。

### 7. Gemini（AI Studio公共接口）

**范围：** OpenAI兼容Chat；原生generateContent/Interactions作为不同协议范围。

**主要参数与特点：** reasoning_effort与google.thinking_config互斥，none是否能关闭依型号。tool_calls[].extra_content.google.thought_signature需原样续接，顺序/平行工具各有签名分布；工具模式/结构化/媒体不能从原生文档直接套到Chat。

**当前代码状态：** 已有公共来源识别和部分thinking互斥规则。native可保留工具签名，但opaque_history/artifact提取不覆盖该嵌套形态；转换明确拒绝extra字段。

**注意事项与未知边界：** 个别指定型号SSE缺index/结束stop有OPEN上游issue，尚未本轮复现；不能全局补0。原生SDK协议/状态和公共Chat不同。

**官方来源：** [major:GE01 · Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)（read）；[major:GE02 · Thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures)（read）；[major:GE03 · Gemini function calling](https://ai.google.dev/gemini-api/docs/function-calling)（read）。

### 8. MiniMax

**范围：** MiniMax-M3 / M3.1-Flash；Chat与Messages分别有合同。

**主要参数与特点：** M3省略thinking：Chat开、Messages关；M3.1始终思考。预算含思考。完整assistant/reasoning/tools历史续接；reasoning_split与think标签为不同输出方言，部分sampling接受但无效。

**当前代码状态：** 已有validate_minimax；跨协议省略默认尚未物化，可能改变有效语义。已有的严格转换不能代替方言识别。

**注意事项与未知边界：** 不能用通用正则删除think文字；schema/工具强制/采样生效和媒体须逐型号逐协议核实。

**官方来源：** [ecosystem:E-D01 · MiniMax OpenAI SDK](https://platform.minimax.io/docs/api-reference/text-openai-api)（read）；[ecosystem:E-D02 · MiniMax Anthropic SDK](https://platform.minimax.io/docs/api-reference/text-anthropic-api)（read）。

### 9. MiMo

**范围：** V2.6 flash/pro/pro-ultraspeed；Responses已读；Chat仅官方索引部分证据。

**主要参数与特点：** Responses不支持previous_response_id/background/context_management；none关思考，其他effort当前等效开启。非auto工具选择可能被移除；完整reasoning_content工具历史回放；max_completion_tokens含思考，采样有固定行为。

**当前代码状态：** 已有状态与非auto工具的防语义损失门禁。Chat索引列developer，不能照抄DeepSeek角色降级。

**注意事项与未知边界：** Chat完整schema未取得，不标全读；未来effort分化和模型变更需追踪；不由HTTP200认定工具参数生效。

**官方来源：** [ecosystem:E-D03 · MiMo Responses API](https://mimo.mi.com/docs/en-US/api/chat/responses)（read）；[ecosystem:E-D04 · MiMo Deep Thinking](https://mimo.mi.com/docs/en-US/quick-start/usage-guide/other/deep-thinking)（partial）；[ecosystem:E-D05 · MiMo Chat API](https://mimo.mi.com/docs/api/text-generation/openai-api)（partial）。

### 10. 硅基流动 / SiliconFlow

**范围：** 平台托管模型合同；仅明确模型名单，不推定原厂合同。

**主要参数与特点：** max_tokens仅答案、thinking_budget独立；effort映射、采样和工具依平台模型。usage提供reasoning/cache细分；OpenAI形状不证明原厂相同默认。

**当前代码状态：** 已有AnswerOnly预算与明确模型校验。

**注意事项与未知边界：** 新托管型号/版本、媒体/schema/缓存价格和工具强度需逐项证据及真实验收，不能按家族自动通过。

**官方来源：** [ecosystem:E-D06 · SiliconFlow Chat API](https://docs.siliconflow.cn/docs/api/chat-completions-post)（read）。

### 11. 火山方舟 / Ark

**范围：** 固定Seed版本与Seed-Evolving滚动别名；Chat/Responses/Messages；10-22升级通知部分已读。

**主要参数与特点：** Chat assistant.encrypted_content、Responses reasoning.encrypted_content、Messages thinking.signature为不同opaque形态；summary非完整思考。滚动别名、采样/effort和cache稳定前缀影响行为。部分流usage已移至内容终帧，坏加密输入未必被上游拒绝。

**当前代码状态：** 现有参数合同不足以覆盖新Chat加密历史提取；静态缺口未实测绕过。Chat参考页仅JS壳，未计完整schema。

**注意事项与未知边界：** Agent旧指南与升级通知对坏签名行为有冲突，需版本限定；不能依赖上游报错替代TokenGo来源归属。

**官方来源：** [ecosystem:E-D12 · Ark Agent invocation practice](https://docs.volcengine.com/docs/ark/agent-model-invocation-practice?lang=zh)（partial）；[ecosystem:E-D13 · Ark Seed-Evolving](https://docs.volcengine.com/docs/ark/latest-model?lang=zh)（read）；[ecosystem:E-D14 · Ark API compatibility upgrade notice](https://docs.volcengine.com/docs/ark/api-schema-upgrade-notice?lang=zh)（partial）；[ecosystem:E-D15 · Ark Chat reference](https://docs.volcengine.com/docs/ark/chat-api?lang=zh)（failed）。

### 12. Ollama

**范围：** 运行时版本与模型metadata共同限定；OpenAI兼容Responses>=0.13.3且无状态。

**主要参数与特点：** OpenAI子集；think/reasoning取值和默认依/api/show与模型；Responses不支持有状态句柄。Local服务忽略上游APIKey不等于TokenGo自身免鉴权。

**当前代码状态：** 已有状态与think门禁；精确运行时指纹/模型修订能力获取仍有限。

**注意事项与未知边界：** 模板、工具/schema/media、采样、token cap与模型revision必须记录；不能按localhost/端口/同名模型推断。

**官方来源：** [ecosystem:E-D07 · Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)（read）。

### 13. vLLM

**范围：** 部署版本、chat template、reasoning/tool parser、generation_config共同限定；stable文档为移动目标。

**主要参数与特点：** Chat需模板；generation_config可改变默认sampling/cap。parallel=false限制调用数，true不保证模型能并行；cache_salt非身份认证。

**当前代码状态：** 已有扩展参数门禁，缺少完整部署指纹闭环。

**注意事项与未知边界：** 运行时/parser的SSE、工具分片、usage/cache、媒体/schema并不由模型名确定；不能为发现能力开启开发调试接口。

**官方来源：** [ecosystem:E-D08 · vLLM OpenAI-compatible server](https://docs.vllm.ai/en/stable/serving/online_serving/openai_compatible_server/)（read）；[ecosystem:E-D09 · vLLM online serving](https://docs.vllm.ai/en/stable/serving/online_serving/)（read）。

### 14. xAI

**范围：** 当前Responses/Grok4.3与4.7，Chat为legacy；不得泛化全部Grok。

**主要参数与特点：** effort默认与none支持依型号；加密reasoning/服务端工具输出需保留。max_output_tokens只限制可见答案，不含reasoning/functioncall；presence/frequency penalty不支持；store默认true，采样回显不保证实际生效。

**当前代码状态：** 无xAI专项profile；当前Responses统一Total预算语义不能据此宣称已兼容xAI。

**注意事项与未知边界：** servertool计费、state资源、输出预算和有效sampling需独立合同；官方长超时建议不等于自动修改全局时限。

**官方来源：** [major:XA01 · xAI reasoning](https://docs.x.ai/developers/model-capabilities/text/reasoning)（read）；[major:XA02 · xAI best practices](https://docs.x.ai/developers/model-capabilities/text/best-practices)（read）；[major:XA03 · xAI Responses reference](https://docs.x.ai/developers/rest-api-reference/inference/responses)（read）。

### 15. Mistral

**范围：** 指定small/medium/large与托管模型；Chat普通function与connector分线。

**主要参数与特点：** system/user/assistant/tool；random_seed有别seed；max_tokens与输入共受context限制。高推理content/delta可为thinking+text列表后转字符串；connector有choices[].messages、多消息引用/服务端工具。json_object非schema保证，json_schema另有支持。流usage需include_usage。

**当前代码状态：** 无Mistral专项profile；通用Chat转换只读字符串和choices[0].message，未覆盖thinking块/connector形态。

**注意事项与未知边界：** 精确effort、schema、media、connector状态/费用与多消息流需专门解析；普通Chat不能宣称connector全兼容。

**官方来源：** [major:MI01 · Mistral API reference](https://docs.mistral.ai/api)（read）；[major:MI02 · Mistral reasoning](https://docs.mistral.ai/studio/conversations/reasoning)（read）；[major:MI03 · Mistral connectors in completions](https://docs.mistral.ai/resources/cookbooks/mistral-connectors-05-connectors-in-completions)（read）；[major:MI04 · Mistral known limitations](https://docs.mistral.ai/resources/known-limitations)（read）。

### 16. Cohere

**范围：** 原生/v2/chat；command-a-reasoning-08-2025。

**主要参数与特点：** 角色不声明developer；专有message.content[]/thought块。默认thinking可disabled；strict_tools顶层而非每工具strict。json/schema与documents/tools互斥；tool_choice枚举/采样范围专有。usage.billed_units与tokens不同。

**当前代码状态：** 无CohereV2协议/profile/executor，改baseURL不产生原生支持。

**注意事项与未知边界：** 预算/账单、流终止、RAG/工具、媒体/schema组合需要专有执行器；不能套OpenAI usage。

**官方来源：** [major:CO01 · Cohere V2 Chat reference](https://docs.cohere.com/v2/reference/chat)（read）；[major:CO02 · Cohere tool parameter types](https://docs.cohere.com/v2/docs/tool-use-parameter-types)（read）；[major:CO03 · Cohere reasoning](https://docs.cohere.com/v2/docs/reasoning)（read）。

### 17. OpenRouter

**范围：** 聚合器合同；模型slug后仍有provider路由与回退。

**主要参数与特点：** reasoning_details包含text/summary/encrypted及format，需完整重放；provider.require_parameters默认false，模型名称不保证最终供应方或参数生效。

**当前代码状态：** 没有聚合器profile；对非空reasoning_details的有损转换明确拒绝是现有保护。

**注意事项与未知边界：** 鉴权后仍需route/provider身份、缓存/cap/usage/tool费用来源合同；不能将聚合器slug强行解释为原厂能力。

**官方来源：** [ecosystem:E-D10 · OpenRouter Reasoning Tokens](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens)（read）；[ecosystem:E-D11 · OpenRouter Provider Routing](https://openrouter.ai/docs/guides/routing/provider-selection)（read）。

### 18. Azure OpenAI / Foundry

**范围：** Azure部署名、地区与API版本；公共OpenAI仅作模型资料，非同一来源身份。

**主要参数与特点：** /openai/v1/responses；APIKey或Entra；v1与日期api-version/preview不同。模型能力随部署对应型号，角色/reasoning/tools/schema/sampling/cap必须绑定部署。

**当前代码状态：** 没有Azure部署身份与版本专用合同；通用透传不证明Entra刷新或部署能力。

**注意事项与未知边界：** media/cache/SSE/usage与服务端资源逐部署确认，访问权限/地区错误不能当公共模型无能力。

**官方来源：** [root:ROOT-AZ01 · Azure OpenAI Responses](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/responses)（read）；[root:ROOT-AZ02 · Azure v1 lifecycle](https://learn.microsoft.com/en-us/azure/foundry/openai/api-version-lifecycle)（read）。

### 19. Vertex / Google托管OpenAI兼容

**范围：** Google托管项目/位置/认证，与AI Studio公共Key分线；所读旧链接已重定向。

**主要参数与特点：** 文档列支持字段集合，未支持可能被忽略；extra_content thought_signature/工具schema需专门处理；项目与区域身份独立。

**当前代码状态：** 当前没有云项目/区域/令牌刷新和部署合同的完整集成；显式profile路径不等于已实现原生托管支持。

**注意事项与未知边界：** model权限、sampling/cap、媒体/schema、状态与usage受云托管合同影响；禁止将被忽略参数写成已生效。

**官方来源：** [root:ROOT-VX01 · Google托管OpenAI兼容](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/migrate/openai/overview)（read）；[root:ROOT-VX02 · Google托管凭据](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/migrate/openai/auth-and-credentials)（read）。

### 20. Amazon Bedrock

**范围：** Converse/ConverseStream、InvokeModel与OpenAI兼容API分别计子线路。

**主要参数与特点：** Converse有system/messages/inferenceConfig/additionalModelRequestFields和独立事件；reasoningContent签名与历史保持。客户端/服务端/Anthropic模式工具须分别处理，不能把servertool当普通function。

**当前代码状态：** 只有Chat/Responses/Messages通用协议不构成Converse executor；新增profile不能补齐AWS原生传输。

**注意事项与未知边界：** AWS鉴权、地区/modelID、EventStream停止与metadatausage、工具结果/媒体/cache/schema依线路与模型核验，均未本轮实测。

**官方来源：** [root:ROOT-BR01 · Bedrock Converse](https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference.html)（read）；[root:ROOT-BR02 · Bedrock tool use modes](https://docs.aws.amazon.com/bedrock/latest/userguide/tool-use.html)（read）。

## Issue与经验的证据等级

本轮索引包含16个唯一issue/PR URL。它们提供重现场景与回归素材，不替代提供方规范，也不证明TokenGo本地有同一故障。closed、merged、进入release分别记录；未查明修复发行版统一为`unknown`。详细状态、提交者所报版本与维护者意见见 [issues.json](issues.json)。

Google #2868为open调查项；Anthropic #1940为closed/not_planned且维护者否定合成前提；OpenAI #3009为duplicate且要求完整历史。Vercel PR #20253已merged但发布版本未知；LiteLLM PR #28134明确merged=false，不能据merge_commit_sha宣布已合并。关闭foreground流与取消background资源也不是同一种执行/计量行为。

## 收集统计与边界

共73条来源记录，71个唯一URL；证据类型与read/partial/failed分布见 [collection-summary.json](collection-summary.json)。源码快照记录不占厂商数。没有把OAuth/API子线路重复计为新来源，没有把未接入的xAI、Mistral、Cohere或云原生协议记为运行通过。

本册是公开资料的有界释义与源码状态索引，不含凭据、用户日志、完整issue正文或全站镜像。未列出的模型、地区、账号、API版本、部署模板与parser组合仍属未知。后续开发缺口、优先级、实施方案由独立方案文档裁决，本册不扩大实施授权。
