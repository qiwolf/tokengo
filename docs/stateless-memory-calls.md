# 一次性记忆抽取调用

需要独立生成结果、不会续传加密推理历史的客户端，可以在原生 `/v1/responses` 请求中增加：

```http
X-TokenGo-History-Mode: stateless
```

此声明只影响本次请求，不改变 Key 或其他请求的策略。TokenGo 不为本次请求创建会话绑定或登记加密推理材料的归属，因此同一 Key 下已有匿名历史池满额也不会阻断这类调用。路由授权、请求日志、用量与账务处理保留。

无状态请求不接受显式会话 ID、加密推理历史、`previous_response_id`、`conversation`、`store:true` 或 `include:["reasoning.encrypted_content"]`。`store` 未提供时设置为 `false`；原生返回内容保持协议兼容，但其中未登记的加密材料不能用于后续历史续接。普通消息及 `function_call` / `function_call_output` 的完整上下文重放仍可使用。

该请求头必须只有一个值 `stateless`，且只适用于 Responses。TokenGo 消费这个内部控制头，不把它传给模型供应商。它不是根据 User-Agent、模型、Key 名或 `store:false` 自动推断的。

已有调用不带此头时，原有历史归属、容量限制和 Key/来源隔离保护保持不变。无需删除历史记录或更换密钥。若客户端需要把加密推理历史带入下一轮，请保持普通模式。

部署调用端补丁时先更新 TokenGo，再更新需要明确无状态的调用端；回滚时先移除调用端请求头，再恢复旧服务端。旧版本可能忽略未知请求头，不能把发送了请求头当作服务端已经启用无状态行为。
