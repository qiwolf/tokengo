# 通用透传与 MinerU 同步接入交付计划

2026-10-21更新：用户已授权实施，并要求先在237完成真实调用验收，再推送GitHub并发布二进制和容器。此前“只做本地、不部署”的阶段限制已被本轮授权替代；不需要再次批准设计。合同见[设计](../specs/2026-10-21-passthrough-design.md)和[完整开发文档](../../passthrough-development.md)。本轮源码 `d5660992a6a1ef0b320ff143a19404ac3abad23e`，[v0.34.0-qiwolf.6](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.6) 已公开。此计划的勾选是证据状态，不能代替实际执行；已发布原生包/镜像的最后一轮隔离运行亦通过，环境为macOS Docker Linux AMD64仿真。

## A. 已有基础

- [x] 固定LiteLLM参考commit与许可证，独立Rust原始HTTP实现。
- [x] SQLite/schema39、加密Header、独立计费渠道、root管理API和面板CRUD。
- [x] 前缀匹配、路径ACL、rawstream、预算/并发/TPM/unknown结算、手动root诊断。
- [x] 已有本地mock/HTTP传输、WASM及合成浏览器证据；Face/CLIP有限真实端点隔离验证。
- [x] MinerU健康/OpenAPI/同步兼容源码只读评估；确认files字段、内联Markdown及当前无URL改写需求。
- [x] 落盘可执行开发合同、237验收和回滚步骤；不把先前证据推广为本轮已验收。

## B. 本轮开发与比例验证

- [x] 核对Key与有效后代授权、实时父链策略及既有对应回归；保留继承语义，路径仍精确匹配。
- [x] 只补MinerU同步接入所必需缺口；不扩展异步/dynamicACL/URL重写，不自动改doc-extract默认入口。
- [x] 必要回归及GitHub CI通过：998项Rust通过、0失败、2忽略，13项Python和WASM通过；已有合成浏览器证据保持其范围。
- [x] 已完成关键边界审查及问题修复；最终源码摘要、命令、退出码和测试证据保存在交付记录。

## C. 237实际部署和真实调用（公开发布前门槛）

- [x] 实时核对目标与现运行版本，停止写入后冷备DB/主密钥/outbox及匹配前后端，验证回滚入口。
- [x] 本地提交固定源码身份（不向GitHub推送），同一提交构建服务端和WASM，部署独立版本目录，确认schema39和基础功能。
- [x] 创建隔离身份/专用VKey及MinerU停用路由；只允许/file_parse和/health，低并发、明确超时/body/价格；核对后启用。
- [x] root probe健康通过，随后带VKey的健康GET与合成单页PDF multipart上传真实通过；确认上游命中、completed和预设Markdown内容。
- [x] 237验证未认证/错误Key/未授权Key及维护/异步路径拒绝、VKey不能管理；ops拒绝由Rust回归覆盖。6笔数据面完成账各50microUSD，3笔root probe不扣用户预算，最终无预占。
- [x] 已核对迁移一致性及启动运行态，保留冷备与验收证据；临时Key、路由、用户及网关会话已清理，无永久路由保留，未改客户端默认流量。
- 237未对临时路由另作重启持久化复测，未执行回滚；持久化回归与分发产物隔离检查按各自证据范围解释，不补写未执行事项。
- [x] 给出237验收结论。失败时先停用新路由保全账务并修复或回滚，**不得继续公开发布**。

## D. 验收后公开双分发

- [x] 源码`d5660992a6a1ef0b320ff143a19404ac3abad23e`已推送并公开[v0.34.0-qiwolf.6](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.6)；对应说明及文档持续同步，公开内容不附现场秘密。
- [x] 该commit的main CI成功；release工作流已构建并公开同commit原生包、WASM、离线镜像及GHCR容器。
- [x] 已完成公开9资产校验及匿名原生包下载、GHCR匿名拉取；同commit构建及工作流校验通过。
- [x] 已发布原生包CLI/server/WASM及透传CRUD/密钥掩码/内部渠道隐藏/ops边界及既有90项能力清单、令牌签发/身份/撤销回归通过；镜像非root/只读/ready/重启/同卷重建通过。范围为macOS Docker AMD64仿真，无生产上游访问。
- [x] 本文和开发文档已同步发布版本、commit、237验收、产物获取/隔离运行和剩余边界；digest及完整现场证据由主交付记录保存。旧版和冷备保留；doc-extract或其他调用方未切流。后续文档补提不改变q6已验证的运行时代码身份。
