# LiteLLM JSON 严格子集导入

这是迁移辅助接口，不是完整的 LiteLLM 配置解释器。仅 root 面板会话可以 POST `/api/import/litellm/preview` 或 `/api/import/litellm/apply`；管理 VKey 也不能调用。请求体是 YAML 经受保护工具转换后的 JSON。服务端不解析 YAML、不展开环境变量。

```json
{
  "import_options": {"name": "personal-gateway", "routing_mode": "ordered_fallback"},
  "model_list": [
    {"model_name": "my-model", "model_info": {"id": "deployment-1", "mode": "chat"},
     "litellm_params": {"model": "openai/Exact-Upstream-Name", "api_base": "https://example.invalid/v1", "api_key": "RESOLVED-SECRET"}}
  ],
  "litellm_settings": {}, "router_settings": {}
}
```

`name` 是稳定导入批次标识（ASCII 字母、数字、`_`、`-`，最多80字节）；`model_info.id` 是批次内稳定且唯一的 deployment 标识。更改它们将被视为不同导入/来源。`model_info.mode` 必须明确为 `chat`、`embedding` 或 `rerank`，不按模型名猜测。逻辑名拒绝控制字符、空名字、保留 auto 和归一化碰撞；upstream 名保持精确符号与大小写。

支持 `openai/<exact>`（声明 Chat 原生协议，embedding/rerank使用各自端点）和 chat 类型的 `anthropic/<exact>`（声明 Messages 原生协议，api_base未以 `/v1` 结尾时添加 `/v1`）。不推断 Responses，也不启用协议转换。原生 Ollama `/api/chat`、`/api/generate` 与本项目协议不同，因此 `ollama/`、`ollama_chat/` 被明确阻塞；已确认支持 OpenAI 兼容接口的 Ollama 服务可显式改写为 `openai/<exact>` 并提供其 `/v1` 地址。

同一逻辑名可出现多个 deployment，按数组顺序生成 per-model 来源白名单；必须显式确认 `routing_mode: ordered_fallback`，因为 LiteLLM 默认调度不保证与此相同。导入设置零重试、关闭主动探活、排除 auto；故障分类遵循 Tokengo 当前网关，不能宣称与 LiteLLM 完全一致。每 deployment 生成独立来源，避免同一来源多 upstream alias 冲突。视觉能力默认 false，须后续单独验证配置。

`router_settings.fallbacks` 或 `litellm_settings.fallbacks` 支持 `[{"A":["B","C"]}]`（只可使用一处，防止优先级歧义）。B/C各 deployment额外映射到逻辑A，同时保留B/C原逻辑路由。多层fallback按深度优先顺序展开并保留首次来源，拒绝环、未知模型、重复目标、类型或原生协议不一致。所有其他settings字段仍阻塞，包含context/content-policy fallbacks、status-specific规则、重试、路由策略、预算、缓存等。展开后执行Tokengo故障分类和粘滞顺序，不能声称LiteLLM运行时语义完全等价。配置字段位置及顺序来源参见[LiteLLM官方fallback文档](https://docs.litellm.ai/docs/proxy/reliability)。`litellm_params` 仅接受 `model/api_base/api_key/http_headers`；temperature、timeout、max_tokens、response_format、custom_llm_provider 等其他参数全部阻塞，避免未实现 defaults 机制导致行为丢失。`model_info` 仅接受 id/mode，其他字段也阻塞。未知字段逐项报告索引而不回显其名称或值，以免错误字段名自身含秘密。

预览只读。返回 `supported/applied/deployment_count/issues/semantics`，无输入的 key、header、URL、模型名或 deployment ID。apply 在不支持时返回 `supported:false, applied:false`，不进行任何写入。只有所有校验通过才在单一 SQLite 事务内更新来源、加密密钥/headers、模型映射、类型、路由策略及 provenance 和审计；数据库错误返回固定消息并回滚。真实上游连通性不在 preview 中测试。只在成功提交后重置受影响路由缓存，不触发探活。

同一 name/id 重复 apply 不新增来源；配置变化更新导入所属的来源（包括凭据、地址、headers、能力及该 deployment alias）。provenance 仅持久化标识到来源ID、所属 alias 和路由策略，不存 key 或 header 明文；渠道凭据和headers经 SecretBox 加密。批次移除的 deployment 默认保留原来源并报告数量；人工新增 alias 保留。已有来源的启停、并发上限、响应解包设置保留，重新导入不会擅自启用已停用来源；凭据、地址、headers和协议能力由导入更新，故不要用同一来源承载需要独立生命周期的人工配置。已有非本批次来源使用同逻辑名、或人工修改过所需路由策略时阻塞，防止覆盖人工路由。删除来源造成 provenance 失效时阻塞，需人工审查后修复；本接口没有破坏式清理选项。

迁移验收还需逐模型真实请求、工具/视觉/流式/取消/故障演练及配置参数逐项核对。此模块的合成 HTTP 回归不代表生产 LiteLLM 全量迁移完成。原始 YAML 转换与真实凭据传输须通过受保护的本地或源端流程，勿将配置粘贴到日志、文档、命令行参数或公开报告。
