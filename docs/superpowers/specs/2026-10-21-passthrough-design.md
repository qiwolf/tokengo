# 通用 HTTP 透传与 MinerU 同步接入设计

状态：2026-10-21，通用Rust透传本地实现已有隔离证据；本轮已获授权继续完成 MinerU 同步接入、237真实调用验收及通过后的双分发发布，无需重复设计批准。完整开发合同和验收步骤见[开发文档](../../passthrough-development.md)，面板操作见[使用说明](../../passthrough.md)。

## 交付范围与已实现合同

SQLite为唯一配置源，schema39新增独立路由、加密Header、Key授权及内部计费渠道类型；root面板CRUD及按revision更新/删除，历史账目不因删除路由而清除。路由不进入LLM模型发现/provider回填，Header以既有AES-GCM加密存储，读API不回显值。

数据面使用最长原始路径前缀匹配；较长路由禁用/未授权不能回落到较短路由。剥离前缀后按最终上游精确路径ACL检查，不区分HTTP方法，不支持动态ID模板。客户端使用独立VKey，当前身份、父链、渠道/付费策略和路由授权共同约束；不能用root会话或ops token替代。授权语义须与最终源码及回归一致，不凭“给父Key授权”推定任意子Key均可扩大访问。

原始请求/响应流转发，不解析JSON/multipart/SSE，不重试、跟随重定向、自动解压缩或继承环境代理；过滤hop-by-hop、入站认证与Cookie，再注入服务端Header。普通HTTP方法可用，拒绝CONNECT/Upgrade/WebSocket；URL解析导致path/query/origin变化时明确拒绝，不能静默改写。不是TCP framing或分块边界保真。

用户/Key/祖先RPM并发及预算沿用现有执行器；TPM受限身份拒绝opaque接口，不伪造token。总期限包括排队、上传及响应，下游停读时仍终止并释放。完成HTTP响应（含4xx/5xx）记1次，明确未发送记0，发送后断开/超时/取消记未知并保守扣预留金额；沿用幂等账本/outbox。管理及root手动probe暂不向ops令牌开放。probe无正文、系统诊断账、不扣用户预算；数据面健康调用仍按路由单价。

## MinerU第一阶段

仅接兼容同步服务默认端口5024：前缀 `/mineru/`，`strip_prefix=true`，`allowed_paths=["/file_parse","/health"]`，专用VKey，低并发1–2。客户端上传multipart字段`files`，接收内联Markdown或ZIP；健康GET与上传POST均须加网关VKey。示例上游 `http://mineru-upstream.example:5024`；现场地址和秘密不入公开仓库。

超时建议从业务样本估算；覆盖兼容层900秒等待时给上传/排队/返回留出余量，例如网关1000秒，调用方/代理匹配。body默认20MiB、最多100MiB且包含multipart开销。不得因后端允许200MiB便承诺网关支持。网关取消不等于后端取消，禁止自动重传大任务。

不实现原生异步tasks/uploads/files/jobs、动态ACL、绝对/根相对URL重写、后台任务取消或按文档聚合计费。不自动改变已有doc-extract默认入口；健康和上传已补可选专属凭据输入，实际切换须验证VKey和回退；未配置新变量仍保持旧行为。原生VLM推理端口不是完整PDF解析流程。

## 验收与发布门槛

已有本地mock/native/WASM/面板和有限Face/CLIP真实端点证据，不等于MinerU已完成真实解析或调用方已迁移。最终代码执行必要回归后，部署237，用隔离身份与合成单页PDF，验证健康、真实解析命中/内容、授权拒绝、按次账/unknown token、root probe与普通数据面区别、重启持久化和回滚点。

本地提交用于固定部署源码身份；237验收通过后才向GitHub推送和创建公开Release；同commit生成匹配的Linux原生包与容器及WASM，校验manifest、OCI revision、digest和匿名获取。任一失败停止发布，按开发文档保全账务与冷备回滚，不将待验收写成成功。真实私有IP、SSH、root密码、VKey及测试材料只保留在非公开运维记录。
