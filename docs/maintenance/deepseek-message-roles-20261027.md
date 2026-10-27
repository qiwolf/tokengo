# DeepSeek 消息角色与 Responses 转换

2026-10-27。适用于 TokenGo 明确识别的 DeepSeek 官方地址，或管理员明确选择的 `deepseek` 参数合同；不按模型名称猜测供应商。

## 问题与行为

OpenAI Chat 客户端可发送 `developer`，DeepSeek Chat 的角色集合不支持它；直接透传会导致上游422。DeepSeek来源现将会话开头的开发者文本指令转为 `system`，保持原有system、developer及其内容顺序，不降为user。文本块仅在明确为纯文本时拼成字符串；图片、未知块字段、缺失文本以及会话开始后再插入developer均明确拒绝，失败时不部分改写请求。

这会把OpenAI的system/developer两级指令折叠为目标的system一级，不能称为优先级无损。其他供应商及OpenAI OAuth保持原样；调用方不需要改写消息。

Responses客户端需要来源明确允许转换：原生协议为`chat`、`allow_conversion=true`。通过已有转换器保留instructions、developer、工具调用与回传，并输出Responses JSON或SSE。不能把`responses`虚报成来源原生协议来绕过限制。未授权转换的其他连接仍会拒绝；opaque状态、模型权限等保护保留。

DeepSeek思考模式工具续接还需要明确的`chat_reasoning_replay=true`：该来源要求完整回传此前的reasoning_content。转换器保留纯文本思考历史，不允许将其他供应商的加密思考状态转换为文本。只启用协议转换并不代表所有输入形态均支持。

## 官方依据与其他来源检查

- [DeepSeek Chat](https://api-docs.deepseek.com/api/create-chat-completion/)：system内容为字符串，未列developer。
- [DeepSeek Responses](https://api-docs.deepseek.com/api/create-response/)：原生接口将developer视为user；不能据此宣称保留OpenAI开发者指令语义。
- [DeepSeek思考模式与工具续接](https://api-docs.deepseek.com/guides/thinking_mode/)：带tools时需完整回传reasoning_content。
- [OpenAI Chat](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)：明确支持developer，保持原角色。
- [Claude兼容层](https://platform.claude.com/docs/en/cli-sdks-libraries/libraries/openai-sdk)：有自身system/developer折叠规则；TokenGo转换到Messages的优先级/时序限制不因本修复删除。

同时检查了通义、智谱、豆包、Gemini、Kimi、MiniMax和硅基流动。[通义](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)、[智谱](https://docs.bigmodel.cn/api-reference/模型-api/对话补全)、[豆包](https://console.volcengine.com/ark/region:cn-beijing/docs/chat-api)Chat文档角色集合未列developer，当前透传存在同类合同缺口；这属于文档与实现对比，未做真实调用，不能记为已复现422。其余来源所查页面不足以证明developer语义，保留待验证。本次仅修复DeepSeek，不对所有“兼容OpenAI”来源统一改写角色。

## 验收与回滚

使用隔离HTTP源验证Chat/Responses、JSON/SSE、文本/文本块、别名映射、usage计量、思考与工具历史续接，另验证协议许可、权限、opaque保护及OpenAI原样。现场验收须以真实网关请求和上游结果为准，不能仅凭单元测试或HTTP200认定工具续接成功。

不涉及数据库迁移，schema42不变。生产修改仅包括TokenGo服务端与对应DeepSeek连接的协议能力；DSH、AG/HS不变。升级前保留程序、数据库、主密钥和配置的一致备份。需要回滚时恢复原程序及该连接的原协议能力，不用旧整库覆盖上线后的新写入。
