# 协议与客户端

本分支提供 `POST /v1/chat/completions`、`POST /v1/responses` 和 `POST /v1/messages`。三者均使用用户虚拟密钥（VKey），共享实时权限、限额、预算与逐来源尝试账本；管理面板口令不能用于模型调用。

## 显式来源能力

在渠道编辑中选择真实支持的源协议及是否允许转换。对每个候选来源，支持客户端协议时原生转发；否则仅在该来源允许转换且请求语义可表示的情况下转换。来源顺序仍由路由配置决定。协议名称不保证某供应商支持该协议的全部角色、参数和工具模式。模型映射、必要的限额及安全校验仍会作用于原生请求。

六个方向的转换覆盖文本、客户端函数工具、工具选择、调用 ID 和工具结果。工具参数必须是完整 JSON 对象，不能把不支持的工具、签名、文件或内容块悄悄删除。Responses 输入中的 developer 角色会保留到支持该角色的 Chat 来源；部分 Chat 兼容供应商只接受 system/user/assistant/tool，此时不能据 OpenAI 协议名称推定它们接受 developer。Messages 无法直接表达独立的 developer 优先级。

原生请求保留支持的供应商扩展；跨协议遇到不能表示的推理形态、签名或不透明状态明确失败。适配记录区分兼容规范与原样转发。输入图像或 Chat 内联 WAV/MP3 还要求该源显式声明能力，不能按模型名判断。

## 供应商的原生接口差异

“原生”表示网关保留该协议请求，不代表供应商实现与 OpenAI/Anthropic 的全部语义相同。例如 DeepSeek 官方 Responses 将 developer 输入当作 user；其 Messages 兼容接口也会忽略部分参数。不能把这种供应商限制称为保留了独立 developer 优先级，或用一次成功调用证明参数全等。

DeepSeek 原生 Responses 的渠道 base 为 `https://api.deepseek.com`，由网关追加 `/responses`。其原生 Messages 渠道 base 为 `https://api.deepseek.com/anthropic/v1`，由网关追加 `/messages`；官方 Anthropic SDK 的 base 省略 `/v1`，因为 SDK 自己追加，不能原样套到本网关。将这些独立协议来源与普通 Chat 来源分开声明，避免一个 base 被误用于所有协议。 官方依据：[Responses API](https://api-docs.deepseek.com/api/create-response/)、[Responses 兼容表](https://api-docs.deepseek.com/guides/responses_api/)、[Anthropic 兼容表](https://api-docs.deepseek.com/guides/anthropic_api/)。

## 来源上下文上限与明文推理

来源的 `protocols` JSON 可声明成对的正整数 `max_input_tokens` / `max_output_tokens`，用于普通 API 来源独立于价格的输入/输出上限。原生不透明历史、音频及需要 token 限额的图像，按已声明的完整输入上限保守预占；不能用假的零价格解锁。费用预算存在时，未定价来源仍会被拒绝，未定价与免费分开。修改能力或上限会改变来源代次。

两个数是独立的绝对上限，并不保证能同时装入供应商的联合上下文窗口。比如官方 DeepSeek 给出1M联合上下文和384K最大输出；完整输入上限可保守声明1048576、输出393216，但不能称其可同时容纳1M输入加384K输出。原生参数能力仍依供应商合同；[官方模型上限](https://api-docs.deepseek.com/quick_start/pricing/)应在配置时复核。

Chat↔Responses 可精确保留明文推理：Chat `reasoning_content` 与 Responses `reasoning.content` 中的 `reasoning_text` 对应，不伪装为摘要。支持一段位于文本/工具之前的明文推理，含流式、用量和工具结果回放；摘要、加密内容、签名及无法表示的交错顺序仍拒绝。跨协议回放到 Chat 时还需来源显式声明 `chat_reasoning_replay:true`（要求 native 包含 chat），因为源会输出该字段不代表接受助手历史中的该字段。原生 Chat 透传不受此转换声明限制。

## 请求示例

`TOKENGO_BASE` 为站点 origin，不含 `/v1`。`MODEL` 是已配置且允许当前密钥访问的逻辑模型名。

```sh
curl "$TOKENGO_BASE/v1/responses" \
  -H "Authorization: Bearer $TOKENGO_VKEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"MODEL","input":[{"role":"user","content":"Hello"}],"stream":true}'

curl "$TOKENGO_BASE/v1/messages" \
  -H "Authorization: Bearer $TOKENGO_VKEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"MODEL","max_tokens":256,"messages":[{"role":"user","content":"Hello"}],"stream":true}'
```

Responses 当前要求客户端完整重放 input。`previous_response_id`、服务端 conversation 续接不支持。不要把其他用户、密钥、会话或来源发行的原生不透明内容移入当前会话：网关只保留归属摘要并验证已发行的状态，不保存这些原文作为共享会话。

## 流式、取消与错误

源端必须先产生有效输出才能提交下游流；提交前的空流、错误或超时可尝试获准的下一个来源。已向客户端提交输出后不切源重放，避免重复工具调用。客户端断开会结束持有的并发槽；终态尝试与结算保留，缺失用量继续是未知。

Responses 来源的 `response.completed.output` 若明确为空数组，只有先前 text/function item 的身份、内容、参数及全部逐项终止事件完整一致时，才使用该流的已关闭内容完成转换。缺失/null/非数组 output 不采用此例外；身份改变、漏关闭或非空汇总文本矛盾均拒绝。稀疏路径限定最多128项/内容part及16MiB累计内容，不扩展签名或加密推理转换。未知/opaque sparse item 不采用此例外；缺省 usage 仍未知并由来源独立观察器计量，原生流字节不重建。非空汇总保留既有转换路径，本次不声称它已新增全面 identity 或 item 集合一致性重校验。

生成用量来自真实源，转换层的协议占位字段不能当作财务用量。转换到 Chat 流时请求源包含最终 usage；源仍可不报告，用量及费用因此可能未知。配置来源价格与预算见[身份与计量](identity-metering.md)。

## 已知审核 telemetry 的转换边界

Responses 来源 SSE 转为 Chat/Messages 时，唯一允许不映射的事件是 `response.metadata` 中的 `metadata.openai_chatgpt_moderation_metadata`。它是第一方展示用审核 telemetry，目标协议没有对应表示；转换记录会明确注明此行为，原生 Responses 仍保留原始字节。本规则不表示网关执行了审核或放宽安全控制，也不保存审核原文。

envelope 仅接受 `type`、`metadata` 和可选的 `response_id` / `sequence_number`，metadata 对象必须只有这个已知 key。可选 ID 有值时须有效，已知来源 response ID 时须匹配；sequence 有值时须为非负整数。审核 key 内部遵循 opaque JSON 合同，不按其中出现的 `headers`、`verification`、`reasoning` 等同名数据字段猜测执行行为；其序列化值限64KiB、深度16、节点4096。这个任意 JSON 值不会建立有效输出、usage 或完成状态，metadata-only 或随后源错误仍在提交前失败并可 fallback。

同级未知 metadata、`safety_buffering`、验证建议、headers/turn state，以及其他未知事件仍明确拒绝；签名与加密 reasoning 的既有拒绝不变。已提交内容后遇到这些不兼容语义，返回流内错误，不切源重放。固定适配备注说明转换政策，不声称该请求实际出现过审核 telemetry。

行为依据是精确 [Codex0.149 Responses reader](https://github.com/openai/codex/blob/rust-v0.149.0/codex-rs/codex-api/src/sse/responses.rs)：已知审核值仅 clone 成展示事件；[core 消费者](https://github.com/openai/codex/blob/rust-v0.149.0/codex-rs/core/src/session/mod.rs)仅发送通知。控制性 metadata 有独立消费者，不能据此一起忽略。本地 HTTP 回归只用合成审核值复现公开 envelope 和已观察事件顺序，不使用真实私有审核内容。

## Claude Code 与 Codex

客户端的版本、启用功能和模型能力共同决定兼容性。先固定单来源，分别验证文本、一次真实工具执行、真实标准输出回传和最终答案，再验证 fallback 与取消；本地模拟服务通过不代表真实供应商通过。

客户端可能默认发送 reasoning、developer 指令、缓存控制、追踪字段、命名空间或定制工具。关闭未使用功能可以避免请求不需要的能力，但不能靠删除实际语义来让测试通过。仅支持裸函数工具的来源不能自动承担 hosted web search、computer、文件或多代理工具。

[独立 OpenAI OAuth](openai-oauth.md)使用专门的官方 Responses 来源，具有不同于普通 API Key 的字段限制。已验证的OAuth传输支持 `max_output_tokens`，Chat/Messages转换以及Responses的 `max_tokens` / `max_completion_tokens` 别名均先归一化到该字段，校验整数且至少16以及已核实的来源/计费输出上界；多个上限不同则拒绝。cap包含推理输出，截断以 incomplete/max_output_tokens 或 Chat length返回并保留实际用量。Chat结构化格式映射到Responses text.format，不改变schema或strict；其他字段与不透明状态仍依专门适配边界处理。GPT 模型目录与授权成功均不代替每个实际模型的推理验收。

DeepSeek 独立[Search/Fetch](deepseek-search-fetch.md)是两条 JSON 工具接口，不会自动接管 CLI 的内置搜索工具。向量与重排接口保持原生，不通过对话协议转换。

### Responses 的稳定会话标识与加密 reasoning

`/v1/responses` 按以下优先级识别会话：专用会话头（沿用现有顺序）、顶层 `session_id` / `conversation_id`、`metadata.conversation_id` / `metadata.session_id`、`client_metadata.session_id`。最后一项仅在 Responses 入口参与身份识别。显式 ID 须非空、至多256字节且无控制字符；所有已提供候选均接受类型与边界校验。

`prompt_cache_key` 始终只是缓存路由提示，不用于会话身份或 opaque 产物归属。它沿用至多512字节、无控制字符、允许空白字符串的兼容合同。工具或提示词变化时缓存键可以改变，独立真实会话也可以共用缓存键；两种情况都不会改变真实会话的来源绑定。

Hermes 等客户端使用 `store:false` 时，应在首轮即通过专用会话头或 `client_metadata.session_id` 提供稳定、每会话唯一的真实会话 ID，后续沿用该 ID，完整回放网关实际返回的 `reasoning.encrypted_content` 和工具结果。网关按 API Key、逻辑模型、协议、渠道、上游模型和来源代次隔离绑定，并核对 opaque 产物确由该真实会话绑定签发；不删除 reasoning、不转发到其他来源。

首轮没有真实会话 ID 时可以正常调用，但不能在后续无绑定地回放 opaque history；仅有 `prompt_cache_key` 也不能建立或借用绑定。这类请求返回 HTTP400，保留 `error.type=invalid_request_error`，另加 `error.code=explicit_session_required`。历史会话在升级前没有建立真实会话绑定的，需要新建会话或从不含旧 opaque history 的上下文重新开始；升级不会根据缓存键自动追认旧产物的归属。

已核对 Cyber Hermes `a4973c3f11d9cc92da986cbe150d1e79d094626f` 及官方 `8edc5ea255aaf17b1f1f944bc2dcce52fce06fa8`：其缓存键由逻辑作用域、instructions 和 tools 共同生成，部分 fork 可以继承作用域；普通 custom Responses 不自动发送 `session_id` 请求头。因此 Hermes 对 QLF 的接入还须转发真实的稳定会话 ID。工具或提示词变化后，只要真实会话 ID 不变，仍可沿原绑定回放；新建或旋转会话后，不允许借用旧会话的 opaque 产物。Tokengo 发行包不会自动修改第三方 Hermes 客户端。

## Chat 请求中的 effort 对象兼容

当 Chat 请求需要转换到仅原生支持 Responses 的来源时，兼容客户端使用的 `"reasoning":{"effort":"low"}` 写法，精确映射到 Responses 的 `reasoning.effort`；Chat 标准写法 `"reasoning_effort":"low"` 继续支持。客户端显式档位优先于来源默认，档位仍由来源能力声明校验，不按模型名称猜测。

兼容对象仅允许 `effort` 字段。两个字段同时出现且值相同可合并；值冲突、非字符串档位、空对象及含有无法无损转换的其他 reasoning 属性时，返回明确 400 并且不发送上游。不会静默删除推理参数，也不通过普遍重试 400 掩盖参数错误。原生 Chat 的供应商扩展透传与原生 Responses 对象语义保持原样；这条兼容规则只用于 Chat→Responses 转换，不扩大 Messages 或不透明推理历史的转换范围。
