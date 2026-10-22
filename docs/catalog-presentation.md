# 供应商、接入与模型的展示整理

模型目录按既有用途分类，并提供供应商筛选；多来源模型匹配任一显式绑定的供应商。展示名称和 API 调用名分别显示，搜索同时匹配两者与供应商。供应商卡片按名称稳定展示，卡片内模型按用途与调用名展示；这些排序不改变请求目标顺序。

管理员可在模型详情修改展示名称，留空恢复调用名。名称最长128个字符，不接受控制字符。新增仅 root 可用的接口：

- `PUT /api/model-presentation?model=<API 调用名>`，请求 `{"display_name":"目录展示名"}`。
- `PUT /api/channels/{id}/presentation`，请求同上，仅修改模型接入名称。
- `PUT /api/providers/{id}/presentation`，请求同上，仅修改供应商名称。

后两项不接受空名称；供应商名称保持唯一。三项写入与审计在同一事务内，审计失败则回滚。维护令牌不开放这些路径。

模型展示名称独立保存在 settings，使用规范化的调用名索引。API调用名、上游模型映射、来源/供应商ID、模型类型、权限、可见性、内部调用标志、价格及路由顺序均不变。供应商改展示名称不会更新 config_version；接入改展示名称不会更新 binding_generation、时间戳或凭据。兼容凭证无需因改展示名重新签发。

DeepSeek 对话、搜索/网页提取、Responses 和 Messages 等协议入口保持各自身份；并未按域名合并。重复接入保留来源编号以区分，不因为名称相似删除或改变授权。

本次237命名清单见 `acceptance/2026-10-23-formal-catalog/presentation-plan.json`。其中模型标识是现有配置身份，展示文案不能替代能力探测或承诺上游能力。

## 展示用途与实际路由类型

`PUT /api/model-presentation` 可附带 `purpose`：`chat`、`embeddings`、`rerank`、`image`、`video`、`audio`、`tools`。省略保持原分类，空串清除并回落实际路由类型。这一字段只写 `model_catalog_purpose_v1:` 展示设置，不修改 `model_kinds`。仅保存展示名称时不会覆盖用途。

例如现有 Qwen3 ASR 接入的执行类型仍为 chat，但目录可归入“音频”；DeepSeek 搜索、网页提取归入“工具服务”。详情同时显示实际路由类型，避免把业务用途当作协议能力。目录用途不改变端点支持、请求格式、auto 候选类型或授权。
