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

生成用量来自真实源，转换层的协议占位字段不能当作财务用量。转换到 Chat 流时请求源包含最终 usage；源仍可不报告，用量及费用因此可能未知。配置来源价格与预算见[身份与计量](identity-metering.md)。

## Claude Code 与 Codex

客户端的版本、启用功能和模型能力共同决定兼容性。先固定单来源，分别验证文本、一次真实工具执行、真实标准输出回传和最终答案，再验证 fallback 与取消；本地模拟服务通过不代表真实供应商通过。

客户端可能默认发送 reasoning、developer 指令、缓存控制、追踪字段、命名空间或定制工具。关闭未使用功能可以避免请求不需要的能力，但不能靠删除实际语义来让测试通过。仅支持裸函数工具的来源不能自动承担 hosted web search、computer、文件或多代理工具。

[独立 OpenAI OAuth](openai-oauth.md)使用专门的官方 Responses 来源，具有不同于普通 API Key 的字段限制。特别是官方预览不接受显式输出 token 上限，因此 Claude Messages 的 `max_tokens` 不能直接宣称无损转换到该 OAuth 来源。GPT 模型目录与授权成功均不代替每个实际模型的推理验收。

DeepSeek 独立[Search/Fetch](deepseek-search-fetch.md)是两条 JSON 工具接口，不会自动接管 CLI 的内置搜索工具。向量与重排接口保持原生，不通过对话协议转换。
