# qiwolf.11 公开发行验收

**v0.34.0-qiwolf.11 已发布并通过公开产物独立验收。** 发布时间为2026-10-23 16:22:46 UTC（北京时间2026-10-24 00:22:46）。发行源码 `551f1c1487a1f262725d916cfc3dbfcda874fbe8`，Cargo0.34.0/schema40；[正式版](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.11)、[精确提交CI](https://github.com/qiwolf/tokengo/actions/runs/39982597664)和[发布工作流](https://github.com/qiwolf/tokengo/actions/runs/39984393819)均可核对。

build-test对精确源码执行完整Rust工作区检查：**1144通过、0失败、3忽略**；另**22项Python通过**，native及WASM release构建通过。3项忽略分别为由父测试调用的子进程辅助入口、外部注册表联网检查和需显式凭据的真实上游检查，不把此前真实调用阶段证据算入本轮数量。历史402项向量定向检查与CI重复覆盖均不叠加为唯一测试总数。

公开9个资产已匿名下载并校验。正式原生包 `tokengo-551f1c1487a1-linux-x86_64.tar.gz` 的SHA256为 `65e79c530662c61536e8011d93ad401e3502230d3897e64a5dec819a8ea646f5`；镜像为 `ghcr.io/qiwolf/tokengo@sha256:c25b2bcb1a934b29fd4ae7772294bbc8add810eff2d507ed9dca4288ab4eed17`。这些是公开产物身份，不与本地编译产生的不同hash混用。

build-test Linux x86_64独立检查通过：匿名GHCR实际拉取、离线镜像双标签、无.env固定引用Compose启动、ready/WASM、非root/只读根、重启与同卷重建，以及正式原生包运行、原生和容器Key行为。结果见[结构化收据](receipt.json)。发布目标为Linux x86_64原生及linux/amd64容器，本轮不宣称其他平台验收。

本次公开验收没有访问或升级生产实例；现场此前已接受的热修复继续作为独立部署记录，不能视为已切换至本版公开产物。GPU官方分词器、8192窗口及外部Hermes会话头均为独立现场变更，不由升级网关包自动应用。

标签和资产携带的是构建时文档快照；最终发布后收据维护在main及GitHub Release页面，后补收据不表示既有资产已重新打包。完整证据、日志及下载产物保存在本地任务目录，不公开私人语料、数据库和凭据。
