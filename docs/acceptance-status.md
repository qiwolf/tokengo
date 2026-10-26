# 功能与验收状态

本次发行目标 **v0.34.0-qiwolf.20**：通用历史软窗口回收及Key归属显示，schema42。已完成的定向与私有部署证据、正式发布待办见[发行记录](acceptance/2026-10-26-qiwolf20-release/README.md)；不把准备阶段视为公开完成。

历史正式版 **[v0.34.0-qiwolf.18](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.18)**，发行源码`1b672c8d34ca`，Cargo0.34.0/schema41。完整新建渠道、公开客户端设备OAuth、参数合同与分层HTTP(S)代理已随同一源码发行；[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40181428585)和[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40182556647)成功，9个公开资产及GHCR、配套原生/容器与Key生命周期独立验收通过。build-test完整工作区Rust **1345通过、0失败、3忽略**；Python分别python-container 3项、python-deploy 6项、python-mineru 13项，不与历史定向687项或重复覆盖累加。真实用户授权完成、令牌刷新和真实模型调用仍须逐账号验收。见[版本说明](releases/v0.34.0-qiwolf.18.md)及[发行收据](acceptance/2026-10-25-qiwolf18-release/README.md)。标签快照不移动，后补结果维护于main与Release页面。

2026-10-25另完成[首页、双语指南与84秒介绍视频更新](acceptance/2026-10-25-docs-video/README.md)，产品仍以q18为基线；本次文档和媒体更新不移动公开标签，也不重新打包已发布资产。

上一公开版 **[v0.34.0-qiwolf.17](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.17)**，发行源码`0aced3ddbdcc`，Cargo0.34.0/schema40。包含Luna与DeepSeek推理关闭的可分发兼容、目录effort解析与原生thinking关闭控制，并保留移动端模型分类/筛选紧凑布局。兼容限定已核实的官方来源和精确型号；冲突拒绝，管理员手工列表及默认值保留，客户端显式参数优先，第三方不自动套用。[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40159567667)和[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40159639375)成功；9个公开资产、匿名GHCR拉取、配套原生/容器生命周期及Key验收通过。build-test完整工作区Rust **1253通过、0失败、3忽略**，Python三组分别13、6、3项，重复覆盖不累加。见[版本说明](https://github.com/qiwolf/tokengo/blob/main/docs/releases/v0.34.0-qiwolf.17.md)与[发布记录](https://github.com/qiwolf/tokengo/blob/main/docs/acceptance/2026-10-25-qiwolf17-release/README.md)。标签及包内文档为构建快照，后补收据维护于main与Release页面，不移动已发布标签。

历史公开版 **[v0.34.0-qiwolf.16](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.16)**，发行源码`2702757fe472`，Cargo0.34.0/schema40。包含本地向量/重排轮询、OAuth结构化输出、输出上限与别名、截断终态、日志和重排计量修复。[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40122964321)和[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40124162546)成功；9个公开资产、匿名GHCR拉取、配套原生/容器生命周期及Key验收通过。build-test完整工作区Rust **1239通过、0失败、3忽略**，Python三组分别13、6、3项，重复覆盖不累加。见[版本说明](https://github.com/qiwolf/tokengo/blob/main/docs/releases/v0.34.0-qiwolf.16.md)与[发布记录](https://github.com/qiwolf/tokengo/blob/main/docs/acceptance/2026-10-25-qiwolf16-release/README.md)。标签及包内文档为构建快照，后补收据维护于main与Release页面，不移动已发布标签。

历史公开版 **[v0.34.0-qiwolf.15](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.15)**，发行源码`c3ecd31250f0`，Cargo0.34.0/schema40。统一控制台、系统设置分组和关键弹窗，修正表单、聚焦与错误状态样式；没有业务接口或数据库迁移变更。本地合并后的原生/WASM构建及原生/容器测试通过；精确main CI和发行流程已成功，9个附件与GHCR镜像已公开。已验收UI完成284项有效浏览器检查。标签/包内文档为构建快照，后补收据维护于main和Release页面。见[版本说明](releases/v0.34.0-qiwolf.15.md)与[发布记录](acceptance/2026-10-25-qiwolf15-release/README.md)。

历史公开版 **[v0.34.0-qiwolf.14](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.14)**，发行源码`1386fa2d410e`，Cargo0.34.0/schema40。包含账务管理、模型映射移除与重加、共享候选与执行一致性修复；main CI及发布工作流成功，9个公开资产已发布。build-test完整Rust1182通过、0失败、3忽略，另Python22项；最终发行提交的native/WASM及原生/容器隔离运行已通过。公开9资产匿名校验、GHCR按digest实际匿名拉取、固定SHA Compose及原生/容器独立运行全部通过，见[版本说明](releases/v0.34.0-qiwolf.14.md)和[发行收据](acceptance/2026-10-24-qiwolf14-release/README.md)。237的真实推理与未定价账本验收属于此前私有修复构建，见[生产收据](maintenance/shared-deepseek-live-20261024/README.md)；本次公开发行没有再次升级237。标签/下载包文档为构建快照，后补收据维护于main和Release页面。

更新日期：2026-10-25。本文区分代码实现、隔离验证和真实供应商验收，不提供机器专用部署信息。Cargo版本保留上游0.34.0；fork双版本分发标签、原生包和离线镜像以[GitHub Releases](https://github.com/qiwolf/tokengo/releases)实际附件为准，不从文档标题推定发布完成。

## 当前代码、部署与发布分界

### qiwolf.14 账务与模型管理发行

账务、模型映射生命周期和共享目标选择修复已正式发布，精确发行源码`1386fa2d410ece30e1a24009e21fa9a1979cdd01`。main CI及发行工作流成功，完整测试与最终SHA本地运行通过；公开9资产匿名下载、实际拉取、Compose及原生/容器运行独立验收全部通过。见[发行收据](acceptance/2026-10-24-qiwolf14-release/README.md)。237此前私有修复已完成真实DeepSeek共享调用和未定价用量账本验收，临时配置已清理，历史保留；[生产收据](maintenance/shared-deepseek-live-20261024/README.md)独立记录，本次公开发行未再次升级237。

### qiwolf.13 公开发行与后续生产升级

Chat→Responses effort对象兼容及6项回归已发布。精确main CI和发行CI成功，build-test完整1166通过/0失败/3忽略，另Python22；公开9资产、实际拉取与原生/容器运行通过。237冷备升级及既有Key真实Chat普通/流式/原生Responses验收、授权/Key和两条300×4096真实回退通过；手机/智能体应用内重试未代测。详见[发行收据](acceptance/2026-10-24-qiwolf13-release/README.md)及[独立升级收据](acceptance/2026-10-24-qiwolf13-upgrade237/README.md)。以下q12及更早记录保留各自时间与阶段，不作为当前版本。

### qiwolf.12公开发行与后续生产升级

发行源码 `6bf36fa917cd87209286c88a805ddc2cb6e52669`。[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40000977772)与[发布工作流](https://github.com/qiwolf/tokengo/actions/runs/40001310546)均成功。GitHub对精确发行提交运行完整Rust工作区，**1160通过、0失败、3忽略**。忽略项分别为由父测试调用的子进程helper、外部联网registry测试、需显式凭据的真实云到GPU向量/重排回退测试；不将三项统称为凭据缺失。本轮build-test重新执行**Web125项、Python22项**并通过，native/WASM release构建与本地原生/容器Key及生命周期检查通过。172个后端文件与此前0d0e6e7完整工作区检查的源码逐字节一致，后端沿用该证据；**没有在本轮build-test重新运行完整工作区**，也不将CI、旧检查和重复运行相加。

9个公开资产匿名下载校验通过。公开原生包 `tokengo-6bf36fa917cd-linux-x86_64.tar.gz` 的SHA256为 `aa4b94472ffecf6a5a941f2c05e5e7eb55ccd7b380da4409f2dfe06271eb3393`；GHCR不可变镜像为 `ghcr.io/qiwolf/tokengo@sha256:ac134402e1f4f3af17aa321b63a9e29235f41b597ef8f007dc822c2e6d93c583`。匿名实际拉取、离线镜像双标签、无.env固定引用Compose启动、非root/只读根、ready/WASM、重启/同卷重建、原生运行与原生/容器Key检查均通过。公开产物身份与本地独立构建hash分开，不混用。

公开发行隔离验收阶段没有访问或升级生产237；其后完成独立冷备升级，当时升级为qiwolf.12公开原生包。既有调用、共享入口授权、Key生命周期及300×4096真实上游回退通过，隔离副本清理完成，见[后续升级收据](acceptance/2026-10-24-qiwolf12-upgrade237/README.md)。此前私有 `0d0e6e7` 为升级前状态，不再表示当前运行版本。GPU分词器及8192窗口为独立现场配置，网关包不会自动修改。标签和资产内文档是构建时快照；最终发布后收据维护在main和Release页面，后补文档不表示资产重新打包。 具体忽略项、检查范围及产物身份见[最终收据](acceptance/2026-10-24-qiwolf12-release/README.md)。

### qiwolf.11公开发行与既有热修复

本版范围为 Chatbox 订阅 system 指令、Responses 显式会话、向量/重排真实负载与 v4 有界噪声兼容检查。300×4096 是完整验收样本，不限制用户请求；实际模型窗口和传输预算分别管理。单个来源续验失败不整体停用已核实健康的来源，真实容量拒绝可逐请求回退，原文和候选保留。详见[版本说明](releases/v0.34.0-qiwolf.11.md)与[能力合同](vector-model-capabilities.md)。

现场热修复已有真实长负载、云/GPU故障回退、共享入口授权、账务与清理证据；v4最终定向检查单次402通过。冻结中文集合的新新和新查询/旧索引组合未观察到主要召回指标退化；只对376条文本、46个审定查询成立。GPU官方分词器、8192窗口和外部Hermes稳定会话头是独立现场改动，不由网关包自动部署。完整客户端聊天UAT和调用方超时配置不在上述验收内。

**本版公开发行已独立验收。** 精确提交[CI](https://github.com/qiwolf/tokengo/actions/runs/39982597664)及[发布](https://github.com/qiwolf/tokengo/actions/runs/39984393819)成功；build-test完整Rust工作区1144通过、0失败、3忽略，另22项Python通过，native/WASM release构建成功。9个公开资产匿名下载校验、GHCR实际拉取、离线双标签、无.env固定引用Compose、非root/只读根、ready/WASM、重启与同卷重建、正式原生和容器Key检查通过。详见[最终收据](acceptance/2026-10-24-qiwolf11-release/README.md)。阶段性热修复与重复检查不叠加数量；3项忽略分别为由父测试调用的子进程辅助入口、外部注册表联网检查和需显式凭据的真实上游检查。

本次没有访问或升级生产，已部署热修复不表示现场已切换正式发行产物。标签及资产内文档为构建时快照，最终收据维护在main与Release页面，不追认先前资产已包含后补文档。


### qiwolf.9公开分发与独立验收

历史公开 **[v0.34.0-qiwolf.9](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.9)**，Cargo0.34.0/schema40。发行源码 `8598fc070ca4a91e700d5748947d0343766ad06b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39898866368)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39898890902)均通过。9个公开资产匿名下载校验、GHCR匿名按摘要拉取、原生与容器Key运行检查及无.env固定SHA Compose启动/重启/同卷重建通过。原生包SHA256 `8ef62e86b8f845add9d1404432980afb8776b7071ada89f39a740c11b26cea3a`；镜像 `ghcr.io/qiwolf/tokengo@sha256:a25ef92a226b8921e3339d744e718572f10fd7bf7e7b4258e1e8c77d7aa5250a`。最终发行源码在build-test完整工作区测试为 **1103通过、0失败、3忽略**，另 **22项Python通过**；精确提交GitHub CI独立成功，不把CI、214项历史定向检查或补丁重复运行相加为唯一测试总数。

原生及容器独立临时实例验证本人/管理员Key创建、精确显示、no-store、权限/CSRF/运维令牌隔离、撤销后不可调用及删除后不可读取。原生隔离库另核实AES密文、物理历史与无明文审计，以及hash-only历史409；容器检查不冒充生产库直查。未在本轮访问或升级237，现场Key交付及原生系统剪贴板仍引用各阶段既有证据，不从隔离Key API检查推定全部浏览器验收。标签及公开资产内的文档是发布前源码快照；完整发布后收据保存在main分支与GitHub Release说明中，不代表先前资产已包含后补回执。

完整结构化收据见[本次公开发行验收](acceptance/2026-10-23-qiwolf9-release/README.md)。历史214项定向检查及现场结果见[再次显示与系统剪贴板](acceptance/2026-10-23-key-reveal/README.md)，不与最终数量相加；历史Hermes无密文无法恢复的限制保留。

### qiwolf.8公开分发与独立验收（历史）

历史公开 **[v0.34.0-qiwolf.8](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.8)** 包含共享向量/重排回退、逐来源独立续验、回收站、透传服务授权与目录管理。Cargo0.34.0、schema40。发行源码 `ca008c7acfcc53d49c5c59670dd123fc28c37a5b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39835960738)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39837203892)通过。Linux x86_64 原生包与 linux/amd64 容器已公开，9个资产匿名下载校验、GHCR按摘要匿名拉取、无.env的Compose启动以及原生/容器实际运行和重启/同卷重建通过。镜像 `ghcr.io/qiwolf/tokengo@sha256:7f8517a515c80d25635d0093f28fe3159bc4bd39b1e946c8e147650a815fe3d4`；原生包SHA256 `e0bda84fe4a79fea2be5d9009151c043889a62d1996069af5970486363bbb616`。完整收据见[本版说明](releases/v0.34.0-qiwolf.8.md#发布收据)。标签及公开资产内的文档是发布前源码快照；完整发布后收据保存在main分支与GitHub Release说明中，不能据此声称资产携带本次最终回执。

最终发布源码 `ca008c7acfcc53d49c5c59670dd123fc28c37a5b` 在build-test以完整 `cargo test --workspace -- --test-threads=4` 验证，GitHub Ubuntu默认线程CI另行通过：两者各1095通过、0失败、3忽略，另22项Python通过；不是两者相加的独立测试总数。两组压力测试分别通过。build-test默认16线程完整运行曾出现本机连接和10秒尾部超时；保留原精确计数/状态断言，补充财务队列等待后形成此发布提交，不把隔离机高线程压力失败隐去或解释为所有生产环境的容量结论。

历史功能阶段（不与最终数字相加）：build-test首轮服务器932+Web122=1054通过、0失败、3忽略；追加用途分类定向服务器26+Web123=149通过、0失败。后者有重复覆盖，不合计为唯一测试总数，不宣称完整Cargo工作区或doc测试。已有Linux服务端与WASM发布构建通过；最终合并源码和公开交付产物已有上列独立验证。

现场有限验收包括共享向量1024维、重排有效结果、普通测试用户仅发现共享入口且底层直调403；自动续验逐来源处理，云端失效不整体停用健康且身份未变的备用来源。目录整理前后API调用名、来源/供应商ID、映射、实际类型、授权、价格、执行指纹和回退链保持一致。服务目录、用户/Key服务限制、撤权及临时配置清理有回执；界面验证筛选、音频/工具分类、详情草稿和取消。新用户保存复制另有普通HTTP/macOS系统剪贴板证据。数据库schema40、完整性/FK及结束时运行/未结/预占/outbox均通过核对。原有账号权限不因验收而收窄或扩大。

实际调用使用合成短文本，不证明全供应商、业务材料质量、长期GPU稳定性或全部客户端；展示用途不改变端点能力、auto类型或实际路由。三组源码与现场收据分别见[共享/续验](acceptance/2026-10-23-vector-shared/README.md)、[透传服务](acceptance/2026-10-23-services/README.md)和[目录整理](acceptance/2026-10-23-formal-catalog/README.md)。

### qiwolf.7公开分发（历史发布基线）

上一公开发行 **[v0.34.0-qiwolf.7](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.7)**，源码`684fa862c64bab5b16c438b7fb8db03295c5b8f4`。本次仅修复发布Compose中的固定GHCR SHA镜像引用、移除构建字段以及离线镜像匹配标签，不更改Rust业务逻辑或schema39。[精确提交CI](https://github.com/qiwolf/tokengo/actions/runs/39646529697)与[发布流程](https://github.com/qiwolf/tokengo/actions/runs/39646795983)均通过；本版9个公开资产匿名下载与SHA校验、native manifest提交匹配、离线双标签同config、GHCR匿名version/sha/latest同摘要检查均通过。本版配套Compose无build、无需.env、使用固定GHCR SHA引用；下载后真实启动、非root（UID10001）、只读根、ready和WASM通过。镜像完整smoke含重启/同卷重建，以及原生CLI/server/WASM、root透传CRUD/密钥掩码/内部渠道隐藏、ops90项清单与签发/身份/禁止新透传管理403/撤销401均通过。运行环境为Linux x86_64隔离构建测试机，本次无macOS或跨架构仿真，不从qiwolf.6借用运行结论。固定镜像摘要为 `ghcr.io/qiwolf/tokengo@sha256:92f3b51bf235098d1d68ab76319bd3c0832c7bebb092ceae97d25312339b931a`。详见[本版说明](releases/v0.34.0-qiwolf.7.md)。

### 2026-10-22正式路由与调用方切换（当日历史状态）

截至2026-10-22该次验收，现场实例运行qiwolf.6源码 `d5660992a6a1ef0b320ff143a19404ac3abad23e`、schema39；本轮未升级qiwolf.7二进制。Face、CLIP/OCR和MinerU三条正式路由启用并保留，各自独立VKey只授权本路由与计费接入，禁止LLM模型调用。网关单价0且按次记账，实际GPU成本未知。Face、CLIP文本/图像、OCR标记合成请求均HTTP200，直连/代理正文对等且新账完成。最终数据库integrity检查正常、外键错误0、全实例未结算与预占均0；正式路由与专用Key保持有效。

MT Photos人脸与共享智能识别两项配置已切至网关；其他模型、线程及阈值保留，仅应用配置重载，PostgreSQL/Redis未重启。AI容器原有GPU访问丢失，本轮只重启该容器一次恢复CUDA，随后OCR直连/代理验收通过。没有触发图库重扫，也没有自然后台任务完成的证据；不能把合成调用解释为全部后台流程或长期GPU稳定性验收。

本机doc-extract默认入口已改专用网关与安全凭据文件，健康和上传均鉴权，上传等待1050秒。10项回归、Mac合成PNG真实CLI返回mineru-gpu与标记、两笔完成账通过。该项为本机外部客户端的运维改动，不代表其他机器/MCP/所有外部消费者迁移；MinerU原生异步资源链仍不支持。

### qiwolf.6发布与2026-10-21临时验收（历史证据）

历史正式分发 **[v0.34.0-qiwolf.6 · 通用 HTTP 透传与 MinerU 同步接入](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.6)** 包含[通用 HTTP 透传](passthrough.md)、schema39、独立路由授权/静态凭据/按次账本及手动诊断，并提供可选MinerU同步客户端。完整实现合同见[开发文档](passthrough-development.md)。

已公开发布，源码为 `d5660992a6a1ef0b320ff143a19404ac3abad23e`。[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39557401353)通过：Rust 998通过、0失败、2忽略，另有13项Python客户端测试及WASM检查通过；[Linux AMD64 打包与发布](https://github.com/qiwolf/tokengo/actions/runs/39557540103)通过。9个公开附件重新下载校验通过，原生包匿名下载与GHCR匿名拉取通过，当日版本、提交与latest标签对应同一摘要（latest随后可变）：`ghcr.io/qiwolf/tokengo@sha256:13b327a4d0c308fbdb7734d4e0550f58589aac1bafed57775cb06cd74f64034f`。

下载后原生CLI/服务端/WASM实际运行，以及透传管理、凭据脱敏、内部渠道隔离与运维令牌权限回归通过；发布容器的非root、只读根、就绪、主密钥权限、重启及同卷重建检查通过。本机运行复核采用macOS Docker Linux AMD64仿真，不证明ARM64原生支持或裸机性能。

部署实例已完成schema38→39迁移与业务验证：Face合成图、CLIP合成文本通过网关均返回200，与直连响应正文逐字节一致；MinerU合成PDF同步解析完成，返回验收标记。6条按次业务账单各50 microUSD，3次root手动诊断记系统诊断账，不扣用户预算，结束时预占为0。该日临时测试配置与凭据已清理，当时未保留永久路由或切换调用方；不适用于上述22日正式接入的新状态。以上是合成业务验证，不代表真人识别精度、真实文档质量、长期GPU稳定性或全部客户端验收；MinerU原生异步接口仍未支持。

以下为上一正式分发的独立历史证据，不作为本次新增功能的验证：

正式分发 **[v0.34.0-qiwolf.5 · 智能体运维令牌](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.5)** 的源码为 `b31cb4173ff6c4f169045ffc4c57630f43981362`。[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39525728580)通过（970通过、0失败、2忽略）；[Linux AMD64 打包与发布流程](https://github.com/qiwolf/tokengo/actions/runs/39525744869)通过。9个公开附件已重新下载并校验；原生包实际运行、令牌签发/身份/90项能力/撤销后401，以及GHCR按摘要匿名拉取、隔离容器初始化/重启/同卷重建均通过。下载后本机运行使用 Linux AMD64 仿真，不宣称 ARM64 原生发行或性能验收。

本版新增独立[运维令牌](operations-tokens.md)：单档站点运维权限、默认90天、最多20把有效令牌、独立撤销与审计、明确的方法/路径白名单和诊断脱敏。运维令牌不能代替推理Key、读取现有凭据明文、管理root身份或继续签发令牌。迁移新增schema38；root正常修改登录名/密码撤销其浏览器会话，但不撤销独立运维令牌，安全重置才同时撤销管理会话和全部运维令牌。专项回归覆盖撤销竞态、审计失败回滚与上游回显敏感信息。

测试实例已在公开发布前部署同一业务/Cargo/前端源码并验收schema38；公开发布另外更新了文档和离线文档打包内容，没有再部署该实例。源码匹配不代表运行二进制与公开附件具有相同摘要。上述发布验收使用隔离数据和受控来源，不新增真实供应商付费调用的验收结论。

## 历史版本与阶段验收

以下保留各阶段的精确提交、测试数量和范围，不能作为当前版本的新验收结果。

历史正式分发 **[v0.34.0-qiwolf.4](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.4)** 的源码为 `15c11c00fa31d3a498a5f86e16d23e20f2bb6503`。[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39469007908)通过（944通过、0失败、2忽略）；[Linux AMD64 双分发流程](https://github.com/qiwolf/tokengo/actions/runs/39469063851)通过，原生与离线镜像下载校验、GHCR按摘要匿名拉取和隔离容器初始化/重启/同卷重建均通过。本机再次运行采用 Docker Desktop 的 Linux AMD64 仿真，不宣称 ARM64 原生发行或性能验收。功能代码 `febb2c91ff51516716a4a40c502d7b41316796f9` 已部署至测试实例，前后端同提交，schema37、53表及历史保护核对通过；本轮发布没有更换该测试实例的程序。

本轮前端105项、服务端库及两类集成共305项通过后，追加来源归属迁移、OAuth MIME/gzip/SSE与工具类型的必要定向回归通过。真实 OAuth Responses 最终输入15/输出5 Token，订阅USD成本未知；独立 Search/Fetch 各1次且各记录750 microUSD。三类均系统诊断、不扣用户额度。首次 OAuth 响应解析失败保留账本，修复后独立重验成功，不覆盖失败证据；本轮未重跑960次外部压力。

浏览器检查覆盖可见授权候选、既有兼容授权保留/撤销取消、管理员与普通用户保存、探针字段保存/取消/启停、桌面与390px布局。三类临时探针通过API逐项删除读回；本轮未逐类型重复GUI删除确认。原供应方、模型、价格、普通用户额度与历史保持；临时用户正常归档，测试Key撤销。探针完整合同见[自定义探针](custom-probes.md)。

此前正式分发为 [v0.34.0-qiwolf.3](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.3)，运行代码/原生包/容器均来自 `f2943d9707e16b231af7e291d4688f3e307ccc4f`。GitHub Rust/WASM CI通过（869通过/0失败/2忽略），同提交AMD64原生包与容器CI、本机镜像导入和隔离生命周期检查通过。目标已冷备升级，schema37及全部53表、SQL对象、序列、环境与主密钥保持。现网既有本地供应方已独立整理为一个渠道、5地址、5凭据、6模型映射，原ID、权限、价格和历史保留；安装包不会按IP自动合并其他部署。上线后已使用原有个人Key真实调用自有Chat、Embeddings、Rerank，均200、原来源结算，运行中/未结/预占均0。Rerank未返回usage，账本保持未知；unpriced不等于免费。线上已有Key可见、留空保留密钥、指定Key目录探测和模型输入顺序已检查，390px布局已验证。

前一正式分发 [v0.34.0-qiwolf.2](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.2) 来自 `3d5c448c408664a14ac0c4400c3dd70f63f2694b`（859通过/0失败/2忽略），完成schema36→37及用户/渠道七项界面修复，其历史证据不代替本版新目录接口验收。

上一版另有14项渠道回归及受控loopback Chat/Embeddings/Rerank三个独立端口实际网关验收，精确路径、映射、凭据隔离与三条账本守恒通过；不把它称为真实供应商推理验收。两个旧测试修复HTTP结束即读异步诊断记录的断言竞态，保持严格最终计数与状态，产品记账逻辑未变。以下为历史阶段，不能代替当前精确版本。

前一正式分发[v0.34.0-qiwolf.1](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.1)来自`ad4aea11f93edc8c46d66979d5cc550801b5848a`（852通过/0失败/2忽略），已单独完成schema34→36及960次自有模型新池记账验收；该历史结果不代替本版新功能验证。

- 已实测目标服务升级到 `87e49b58ae6143503d884d0118d9a6885fb1496b`：原生包/配套WASM更新，业务表及schema保护核对保持，MiniCPM一次真实调用输入20/输出1 Token成功。此结果属于87e，不证明随后统一用户页面或容器新版本已上线。
- 后续容器与优雅退出基线 `74c8800aeb7166275575fb0dfa9019c55bc0045a` 已推送；本机Linux ARM64源码镜像完成服务端/CLI/WASM构建及非root、鉴权、实际WASM、主密钥0600、重启持久化和两次正常停止检查。此结果不替代最终AMD64分发镜像CI。
- 第一阶段统一用户、Browser自助Key与点选策略的精确提交 `48960927cceaf67ecc45f8ed57bda38ac8ff5599` 完成Rust/WASM CI、同提交AMD64原生包/容器构建与容器生命周期检查，已部署。冷备前后全部业务表/schema一致，已有浏览器会话可正常打开统一用户页。隔离浏览器已验证普通用户自助创建、使用与撤销Key及390px布局；实际推理使用本机受控来源。
- 新的渠道多Key池和用户私有路由已有23项渠道/池、10项私有路由、20项网关HTTP定向回归通过。真实冷备离线演练schema34→36、36来源不变，旧6分组整理为19入口/20凭据，所有受保护旧表与schema保持，二次backfill幂等；另有10个保护反例被拒绝。本轮全仓852通过、0失败、2忽略；隔离浏览器验证渠道/Key增改停用、虚拟名顺序与每周时段、真实别名调用和账单，以及390px无横向溢出、无JSON编辑。新池路径自有模型960次真实请求全部成功且960账本结算守恒（使用独立本地测试价1 microUSD/Token，不是供应商实付账单）（客户端最高128并发、来源及凭据保护并发4，非128路GPU生成）。同一运行提交的双分发CI、正式Release及目标原生升级现已核验；详见本页顶部与Release随附验收记录。

第一阶段实际没有新增schema、用户整体到期/自助开关，固定32把活动Key；沿用microUSD金额上限，未实现原始Token套餐、月周期或用户/Key policy版本409；自定义探针现已独立实现并带自身配置版本保护。完整覆盖见[需求状态](requirements-status.md)，实际API见[账号与自助Key合同](account-login.md)。旧用户与面板账号兼容API不再是主UI的手工关联路径；root保留独立安全身份。

## 本轮服务商与模型管理

服务商分组、原连接独立保留、模型启停、公开模型清单与兼容别名隐藏已实现。推理档位读取真实目录声明，来源不明时保留未知或由管理员明确确认；客户端原生参数优先。隔离浏览器已验证新增模型即时更新父表、模型停用与恢复、默认档位保存与恢复服务商默认，以及手机 390px 布局。

服务商分组与原生档位功能已部署，并在真实控制台完成一次官方目录读取：`gpt-6.1-sol` 声明 low / medium / high / xhigh / max / ultra，仍使用服务商默认。现有个人 VKey 的模型清单已显示三个标准 GPT 名称、收起十个旧档位别名；未扩大其权限。此处证明目录声明和管理链路，不证明每个档位均已进行推理实测。

发布核对保留了原连接、账号会话、权限、价格、OAuth 绑定、历史账本及环境主密钥；启动期间旧视觉退避记录保持。已修复管理字段改变旧视觉探测指纹的问题并新增兼容回归。初次被拦截的升级曾可能唤醒既有视觉任务，其请求是否完成或收费尚不确定；不能将部署脚本未发推理扩大为全部后台活动均无调用。

历史服务商与原生档位生产基线为 `d8635543cdb7fdbc0cb16a731947bb40b226fc53`，随后 `57382c8` 更新卡片样式；本次已实测升级到上述87e，包含并发写入修复与协议矩阵。统一用户已按4896092单独部署；多Key与私有策略的后续上线不能用上述历史升级结果代替。最终分支 CI、界面资产和审阅状态见 [PR #3](https://github.com/qiwolf/tokengo/pull/3) 与 [GitHub Actions](https://github.com/qiwolf/tokengo/actions)。CI 结论须按提交匹配，不用历史测试数量替代当前结果。

## 已有有限真实验证

| 范围 | 已有证据 | 仍未证明 |
| --- | --- | --- |
| 多用户 / 多 VKey / 账务 | 权限、父级预算、范围隔离、持久预占结算及恢复回归；部署保全核对 | 多实例共享配额、供应商最终账单完全一致 |
| 模型来源 | 原非 GPT 来源中 11 个完成有限用途调用；5 个余额或额度不足由用户明确暂缓 | 所有来源目前可用、任意模型的完整能力 |
| 原生与协议转换 | Chat / Responses / Messages 的可表示文本和客户端函数工具；指定客户端单工具循环与流式修复验证 | 全部客户端插件；签名、加密和任意不透明状态的无损跨协议转换 |
| 独立 OpenAI OAuth | 官方登录授权、目录、有限文本、指定 Codex 工具闭环及 GPT 到 Chat 流式样例 | 任意订阅转换、实际 priority 加速、真实加密推理重放、未到期 token 的强制轮转 |
| DeepSeek Search / Fetch | 各一次真实请求及独立按次入账 | 自动接管客户端内置搜索、供应商最终账单对账 |
| 向量、重排、图像与语音 | 有限原生用途样例 | 完整召回基准、独立音视频或文件 API 的全面转换 |
| 看板 | 真实请求指标、分组与明细、预算价格和逐尝试账本 | 完整成本趋势图或通用 BI 系统 |

真实供应商结果属于其测试提交和当时账号状态，不能用作持续可用性保证。自动识别档位读取目录，不证明供应商每个档位已进行付费推理实测。新模型名称与可选档位以当前授权来源返回的目录为准。

## 运行与安全边界

主流程使用统一用户、账号密码和同源HttpOnly会话；内部凭证与权益分表保留，VKey不等于root管理员。管理员变更登录名或密码须验证当前密码并撤销其浏览器会话；独立运维令牌继续有效，须单独撤销或执行安全重置。现有内网部署不能替代公网 HTTPS 验收；新部署应遵循 [部署指南](deployment.md) 先初始化、固定审阅提交、保留数据库与主密钥的一致备份，再开放访问。

现有 LiteLLM 未被替换或切流。固定 DNS、离机灾备、所有移动端和浏览器的全面验收，以及更多供应商专用订阅适配仍有后续工作。
