# 模型规格与用量参考值

管理员在「模型与路由 → 模型规格」维护官方默认和渠道特例。每个上游型号可记录上下文窗口、输入用量参考值、总输出用量参考值、模型类型、扩展指标和来源链接。扩展指标以名称、整数值和单位保存，可记录向量维度、重排文档数、音视频时长和分辨率等数值。「供应商与接入 → 管理上游 Key → 选择模型」按实际发送的上游模型名匹配。点击「编辑渠道规格」会带入当前匹配值，保存后作为本渠道同名模型的特例。

## 匹配规则

使用顺序为管理员渠道特例、已核对的中转渠道差异、模型默认。只有明确登记的别名共享同一模型默认，例如 `deepseek-v4.1-flash` 和 `deepseek-flash`。已登记的 OpenCode 和 Command Code 差异同时匹配其服务地址和精确上游型号。删除渠道特例后，下一次读取时恢复继承。未知字段留空。

定价使用同一匹配结果。已有价格的参考值随其版本保留。新价格可填入规格表的参考值，缺项仍可从来源配置或供应方目录补充，也可直接在定价表单调整。实际费用按上游报告的用量结算。来源真实容量按原有流程配置与验证。

## 初始规格来源

初始数据是 2026-11-18 核对的可编辑快照。公开目录或模型说明属于供应方声明。已分别公布的输入和输出容量可用作估算表单的初始参考值。对明确为输入和输出共用的窗口，初始输入参考值采用上下文窗口减去公布的最大输出，给输出留出空间。嵌入模型的单项长度进入上下文规格，批量请求的输入参考值按批量场景另填。

| 官方资料 | 纳入型号 |
| --- | --- |
| [DeepSeek 模型接口](https://api-docs.deepseek.com/api/list-models/) | DeepSeek V4.1 Flash、V4 Pro |
| [OpenAI Luna](https://developers.openai.com/api/docs/models/gpt-6-luna)、[Sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol)、[Terra](https://developers.openai.com/api/docs/models/gpt-6.1-terra) | GPT-6 Luna、GPT-6.1 Sol、GPT-6.1 Terra |
| [Claude Sonnet 4.6](https://platform.claude.com/docs/en/models/sonnet-4-6/overview)、[GLM-5.2](https://docs.z.ai/guides/llm/glm-5.2)、[Kimi K2.7 Code](https://huggingface.co/moonshotai/Kimi-K2.7-Code)、[Gemini 3.8 Flash](https://aistudio.google.com/docs/models/gemini-3.8-flash) | Claude、智谱、Kimi、Gemini 各一款 |
| [OpenAI 嵌入](https://developers.openai.com/api/docs/guides/embeddings)、[BGE-M3](https://huggingface.co/BAAI/bge-m3) | 两款 OpenAI 嵌入模型和 BGE-M3 的单项长度与维度 |

| 渠道 | 已纳入差异 |
| --- | --- |
| [OpenCode Go](https://models.opencode.ai/providers/opencode-go/) | DeepSeek V4.1 Flash 和 V4 Pro 在目录中的窗口为 1,000,000，最大输出 384,000。DeepSeek 官方同名模型为 1,048,576 和 393,216。 |
| [OpenCode 按量](https://models.opencode.ai/providers/opencode/) | DeepSeek V4.1 Flash 为 1,000,000 和 384,000。独立免费型号为 200,000 和 32,000。 |
| [Command Code](https://api.commandcode.ai/provider/v1/models) | 已核实的 DeepSeek Flash 普通版、Fast 与 V4 Pro 窗口为 1,000,000。所查 Kimi 普通版和 Highspeed 为 256,000 和 262,000。相关条目没有输出上限，规格中留空。 |

Command Code 的 GPT-6 Luna、GPT-6.1 Sol、GPT-6.1 Terra 与 OpenAI 所列上下文一致，继承同一模型默认。OpenCode 已核实一致的 Luna、Sol 等条目也共用同一默认。核对覆盖上表所列型号和相关变体。

## 客户端接口

使用调用 API Key 读 `GET /v1/models`。按 `data[].id` 匹配客户端的调用名，读取可选的 `data[].specifications`。例如：

```json
{
  "id": "configured-model-id",
  "object": "model",
  "specifications": {
    "kind": "chat",
    "context_window": 1048576,
    "input_usage_reference": 655360,
    "output_usage_reference": 393216,
    "origin": "official"
  }
}
```

同一调用名有多个可用来源时，各来源一致的字段才可随该调用名发布。来源缺规格时，整个 `specifications` 可省略。扩展指标格式为 `"metrics": {"embedding_dimensions": {"value": 1536, "unit": "dimension"}}`。来源标签可能是 `official`、`relay_official`、`maintained` 或 `multiple_sources`。顶层 `context_window` 为另一路来源容量字段。
