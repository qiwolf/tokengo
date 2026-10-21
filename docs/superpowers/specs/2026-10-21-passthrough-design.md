# 通用 HTTP 透传与 MinerU 同步接入设计

状态：2026-10-21，已按源码 `d5660992a6a1ef0b320ff143a19404ac3abad23e` 实施并在237完成schema39迁移、Face/CLIP字节一致性与MinerU合成PDF真实解析验收；[v0.34.0-qiwolf.6](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.6) 已公开。现有调用方未切流、临时路由未保留。公开资产校验和匿名下载/拉取通过；原生包与镜像隔离运行、重启及同卷重建等检查亦通过，环境为macOS Docker Linux AMD64仿真，不访问生产上游。完整开发合同和验收步骤见[开发文档](../../passthrough-development.md)，面板操作见[使用说明](../../passthrough.md)。

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

本轮CI为998项Rust通过、0失败、2忽略，13项Python及WASM检查通过。237三服务真实合成调用已通过：Face/CLIP HTTP200且body与直连逐字节一致；MinerU completed且内联Markdown包含预置标记。6笔完成数据面账各50microUSD，3笔root probe不扣用户预算，最终无遗留预占；fixture已清理。未切换调用方，未实际执行回滚，不能把这一小样本验收当作持续可用性或全部参数验收。

本地提交固定部署源码身份，237验收通过后才推送GitHub并公开Release；本轮已完成。公开9资产校验、原生包匿名下载、GHCR匿名拉取成功；发布产物隔离运行亦通过：原生CLI/server/WASM、root透传管理及ops边界、容器非root/只读/ready/重启/同卷重建，范围为macOS Docker AMD64仿真。同commit生成匹配Linux原生包、容器及WASM；后续部署仍须校验manifest、OCI revision、digest，遇到失败按开发文档保全账务并执行经审阅的回滚，不将待验收写成成功。真实私有IP、SSH、root密码、VKey及测试材料只保留在非公开运维记录。
