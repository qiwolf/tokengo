# 通用透传与 MinerU 同步接入交付计划

2026-10-21更新：用户已授权实施，并要求先在237完成真实调用验收，再推送GitHub并发布二进制和容器。此前“只做本地、不部署”的阶段限制已被本轮授权替代；不需要再次批准设计。合同见[设计](../specs/2026-10-21-passthrough-design.md)和[完整开发文档](../../passthrough-development.md)。此计划的勾选是证据状态，不能代替实际执行。

## A. 已有基础

- [x] 固定LiteLLM参考commit与许可证，独立Rust原始HTTP实现。
- [x] SQLite/schema39、加密Header、独立计费渠道、root管理API和面板CRUD。
- [x] 前缀匹配、路径ACL、rawstream、预算/并发/TPM/unknown结算、手动root诊断。
- [x] 已有本地mock/HTTP传输、WASM及合成浏览器证据；Face/CLIP有限真实端点隔离验证。
- [x] MinerU健康/OpenAPI/同步兼容源码只读评估；确认files字段、内联Markdown及当前无URL改写需求。
- [x] 落盘可执行开发合同、237验收和回滚步骤；不把先前证据推广为本轮已验收。

## B. 本轮开发与比例验证

- [x] 核对Key与有效后代授权、实时父链策略及既有对应回归；保留继承语义，路径仍精确匹配。
- [ ] 只补MinerU同步接入所必需缺口；不扩展异步/dynamicACL/URL重写，不自动改doc-extract默认入口。
- [ ] 运行针对性passthrough管理/传输/计量测试及必要workspace/WASM检查；改动面板时补浏览器复验。
- [ ] 独立审查关键边界，修复确认问题，保存最终源码摘要、命令、退出码、测试证据。

## C. 237实际部署和真实调用（公开发布前门槛）

- [ ] 实时核对目标与现运行版本，停止写入后冷备DB/主密钥/outbox及匹配前后端，验证回滚入口。
- [ ] 本地提交固定源码身份（不向GitHub推送），同一提交构建服务端和WASM，部署独立版本目录，确认schema39和基础功能。
- [ ] 创建隔离身份/专用VKey及MinerU停用路由；只允许/file_parse和/health，低并发、明确超时/body/价格；核对后启用。
- [ ] root probe健康通过，随后带VKey的健康GET与合成单页PDF multipart上传真实通过；确认上游命中、completed和预设Markdown内容。
- [ ] 未认证/错误Key/未授权Key/维护及异步路径拒绝；ops不能管理；健康和解析按HTTP attempt计量，root probe记系统账，token未知且预占释放。
- [ ] 重启验证路由/加密凭据持久化，保留失败/成功证据，清理临时Key/材料，不改已有客户端默认流量。
- [ ] 给出237验收结论。失败时先停用新路由保全账务并修复或回滚，**不得继续公开发布**。

## D. 验收后公开双分发

- [ ] 更新实际发布说明/状态，完成敏感信息检查；提交并推送同一最终源码，再标本轮新版本（不预称某tag已发布）。
- [ ] 该commit的main CI成功；release工作流构建校验同commit原生包、WASM、离线镜像与GHCR容器。
- [ ] 校验bundle manifest、OCI revision、SHA-256、固定镜像digest和版本对应；容器隔离生命周期/持久化检查通过。
- [ ] 匿名获取Release附件并拉固定digest，验证原生包及容器实际启动，不用附件可下载替代运行验收。
- [ ] 汇总发布版本、commit、237结果、产物/digest、证据与剩余边界；保留可回滚旧版。不宣称doc-extract或其他调用方已迁移。
