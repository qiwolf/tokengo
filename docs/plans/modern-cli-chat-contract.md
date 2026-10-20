# 现代 CLI 经 Chat 来源的受控兼容边界

本次验收链路是实际 Codex/Claude CLI → 实际 Tokengo → 仅提供 `/v1/chat/completions` 的受控来源。客户端仍分别发送 Responses/Messages；中间观察代理保留原始请求 body，不删除不支持字段。来源先返回函数调用，CLI 实际执行固定 `printf TOKENGO_TOOL_OK`，下一轮携带正确工具 ID 与真实 stdout，再返回最终文本。它证明文本与函数调用的转换链路，不代表真实收费模型、任意工具或完整原生协议兼容。

## 明确的转换规范

仅跨协议转换采用以下规范；同协议原生请求继续保留原始字段。

- Responses `client_metadata` 必须是对象，只作为客户端追踪提示，转换时不转发、不保存其具体值。
- `prompt_cache_key` 必须为不含控制字符、最长 512 字节的字符串。转换时不转发，Tokengo 不声称实现该缓存亲和性。
- `reasoning` 只接受空对象，或 `effort`/`summary` 明确为 `none`。`auto`、`detailed`、真实 effort、未知参数和错误类型明确拒绝。
- `include` 接受空数组；非空只接受单项 `reasoning.encrypted_content`，且必须同时明确提交上述空/禁用 reasoning。此时只是无启用 reasoning 的客户端通用输出提示，不能产生或承诺 opaque reasoning。真实加密 reasoning 历史仍拒绝转换。
- Messages `output_config` 只接受空对象、空/`none` effort，以及空或仅 `type:text` 的 format。真实 effort、JSON schema 或未知格式明确拒绝。

网关记录固定的规范化说明，说明中不包含 metadata/cache key 值。未支持的语义由转换器返回明确错误；没有在代理层删除字段来规避检查。Chat 来源若实际返回非空 `reasoning_content`，仍不能据此伪造原生 reasoning/encrypted state。

## 实测客户端配置

Codex 使用自定义 provider 的 `wire_api="responses"`，并设置：

```toml
features.multi_agent = false
features.responses_websockets = false
model_supports_reasoning = false
model_reasoning_summary = "none"
web_search = "disabled"
```

只设置 `model_supports_reasoning=false` 仍实测产生 `reasoning.summary=auto`，所以必须同时禁用 summary。关闭 multi_agent 后请求为普通 function tools；namespace 等无法等价转换的工具继续拒绝。本次实际请求仍包含 `reasoning={}`、`include=["reasoning.encrypted_content"]`、`client_metadata` 和字符串 `prompt_cache_key`，由上述规范处理。

Claude 使用与实际逻辑模型名称对应的环境变量，例如测试模型：

```sh
CLAUDE_CODE_MODEL_CAPABILITIES='smoke-chat=-effort'
CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1
MAX_THINKING_TOKENS=0
DISABLE_PROMPT_CACHING=1
```

并设置 `alwaysThinkingEnabled:false`。单独关闭 thinking 或将 `effortLevel` 设 null，实测仍发送 `output_config.effort=high`；明确禁用该模型的 effort capability 后才不发送它。替换模型名时须同步修改 capability 配置。受控验收只开放 Bash 的固定 printf 命令；不能推断客户端全部内置工具可用。

## 验收证据与隔离

仓库外 `review/route-c/ops/client_chat_conversion.py` 在临时数据库、临时客户端 HOME/配置目录、固定合成密钥和 loopback mock 中运行，未访问生产来源或读取真实客户端密钥。外层 macOS sandbox 仅允许 loopback 网络并拒绝 securityd。Codex 内层 sandbox 与该外层 seatbelt 嵌套会报 `sandbox_apply: Operation not permitted`；测试关闭了内层 sandbox，仍保留外层隔离。这是受控工具执行环境配置，不是生产客户端 sandbox 建议。

最终证据保存在仓库外 `review/route-c/evidence/{codex,claude}-chat-conversion-final.json`。通过条件同时要求：CLI exit 0、恰好两轮 Chat 上游请求、正确工具 ID、真实工具 stdout marker、最终答复 marker、无来源协议错误。只有最终文本或 HTTP 200 不算通过。


转换到 Chat 的流式请求会显式发送 `stream_options.include_usage=true`，请求上游原生用量。仅收到模型实际报告的 meter 才标 reported；源仍未报告时保持 unknown，不用 Messages/Responses 转换目标的零占位值结算。原生 Chat 请求保持原样。
