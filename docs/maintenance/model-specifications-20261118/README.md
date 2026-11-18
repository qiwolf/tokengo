# 模型规格与用量参考值

管理员在「模型与路由 → 模型规格」维护官方默认和渠道特例。每个上游型号可记录上下文窗口、输入用量参考值、总输出用量参考值、模型类型、扩展指标和来源链接。扩展指标以名称、整数值和单位保存，可记录向量维度、重排文档数、音视频时长和分辨率等数值。「供应商与接入 → 管理上游 Key → 选择模型」按实际发送的上游模型名匹配。点击「编辑渠道规格」会带入当前匹配值，保存后作为本渠道同名模型的特例。

## 匹配规则

使用顺序为管理员渠道特例、已核对的中转渠道差异、模型默认。只有明确登记的别名共享同一模型默认，例如 `deepseek-v4.1-flash` 和 `deepseek-flash`。已登记的 OpenCode 和 Command Code 差异同时匹配其服务地址和精确上游型号。删除渠道特例后，下一次读取时恢复继承。未知字段留空。

定价使用同一匹配结果。已有价格的参考值随其版本保留。新价格可填入规格表的参考值，缺项仍可从来源配置或供应方目录补充，也可直接在定价表单调整。实际费用按上游报告的用量结算。来源真实容量按原有流程配置与验证。

## 初始规格来源

初始目录收录 2026-11-18 可从公开服务使用的 96 款主流型号，包括原有的 12 款和本轮新增的 84 款。已开放的预览和仍提供调用的常用旧版按精确型号列入。目录适用于新接入渠道，已有数据库通过增量迁移补录。公开目录或模型说明属于供应方声明。已分别公布的输入和输出容量可用作估算表单的初始参考值。对明确为输入和输出共用的窗口，初始输入参考值采用上下文窗口减去公布的最大输出，给输出留出空间。嵌入模型的单项长度进入上下文规格，批量请求的输入参考值按批量场景另填。

| 官方资料 | 家族或用途 |
| --- | --- |
| [OpenAI](https://developers.openai.com/api/docs/models/all)、[Anthropic](https://platform.claude.com/docs/en/models/overview)、[Google](https://aistudio.google.com/docs/models)、[xAI](https://docs.x.ai/developers/release-notes) | GPT-6.2/6.1/6/5.x，Claude 5.5/5/4.x 与 Fable，Gemini 4/3.x，Grok 4.x |
| [DeepSeek](https://api-docs.deepseek.com/api/list-models/)、[Qwen](https://help.aliyun.com/en/model-studio/text-generation-model/)、[智谱](https://docs.z.ai/guides/llm/glm-5.3)、[Kimi](https://platform.kimi.ai/docs/models) | DeepSeek V4，Qwen4/3.8/3.7，GLM 5.3/5.2/5.1，Kimi K3/K2 |
| [MiniMax](https://platform.minimax.io/docs/api-reference/text-openai-api)、[小米 MiMo](https://mimo.mi.com/docs/quick-start/model)、[阶跃星辰](https://platform.stepfun.ai/docs/en/guides/models/step-5-preview)、[Mistral](https://docs.mistral.ai/models/mistral-large-4-0) | MiniMax M3、MiMo V2.6、Step 5/3.x、Mistral Large 4/Small 4 |
| [Cohere](https://docs.cohere.com/docs/cohere-embed)、[百炼向量和重排](https://help.aliyun.com/en/model-studio/embedding-rerank-model/)、[BAAI 重排](https://huggingface.co/BAAI/bge-reranker-v2-m3) | 向量、重排；另收录代表性语音、图像、视频和工具型号的专用指标 |

完整型号、精确值与逐条来源在 [首次预置](../../../crates/server/src/db/model_specification_defaults.sql) 与 [公开目录扩充](../../../crates/server/src/db/model_specification_catalog.sql)。如果官方分别声明输入上限、输出上限与共享窗口，实际请求还需满足窗口约束。没有足够依据的字段保留为空，局部推理开关、区域和协议差异以对应服务文档为准。

| 渠道 | 已纳入差异 |
| --- | --- |
| [OpenCode Go](https://models.opencode.ai/providers/opencode-go/) | DeepSeek V4.1 Flash 和 V4 Pro 在目录中的窗口为 1,000,000，最大输出 384,000。DeepSeek 官方同名模型为 1,048,576 和 393,216。本轮补录 GLM 5.2、5.3 及 5.3 Flash 的输出差异。 |
| [OpenCode 按量](https://models.opencode.ai/providers/opencode/) | DeepSeek V4.1 Flash 为 1,000,000 和 384,000，独立免费型号为 200,000 和 32,000。本轮补录 Qwen3.8 Max、MiniMax M3 和 MiMo V2.6 Flash 免费版的差异。 |
| [Command Code](https://api.commandcode.ai/provider/v1/models) | 已核实的 DeepSeek Flash 普通版、Fast 与 V4 Pro 窗口为 1,000,000。所查 Kimi 普通版和 Highspeed 为 256,000 和 262,000。本轮补录 GPT-5.5、GPT-5.4、Kimi K3、Kimi K2.6。上述目录条目缺少输出上限，规格中留空。 |

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
