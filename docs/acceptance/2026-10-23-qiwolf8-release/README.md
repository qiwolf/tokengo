# qiwolf.8 公开发行验收

源码与发行标签：`ca008c7acfcc53d49c5c59670dd123fc28c37a5b` / `v0.34.0-qiwolf.8`。

[公开下载](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.8)；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39835960738)；[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39837203892)。本目录为发布后独立收据，标签内文档是发布前快照。

构建与测试执行于 build-test，Mac 保留源码主仓库。完整 workspace：1095通过、0失败、3忽略；Python22通过。默认16线程聚合运行曾发生本机连接和财务尾部超时，原日志保留；完整运行用4测试线程通过，两组内部最高128并发压力用例单独通过，GitHub默认线程CI亦通过。测试次数不叠加。

9个实际公开资产匿名下载并校验，服务端、CLI、WASM与源码身份核对通过；GHCR按摘要匿名拉取、实际离线包载入、无.env的Compose启动、非root/只读根、重启与同卷重建通过。平台仅Linux x86_64 / linux/amd64。本次公开产物核验使用隔离实例，没有访问或升级237。

收据字段以JSON为准：`verification-q8.json`总结果、`assets-q8.json`附件摘要、`registry-tags-q8.json`镜像摘要与标签、`offline-tags-q8.json`离线镜像别名、`native-runtime-q8.json`原生运行、`image-smoke-q8.json`容器生命周期、`test-summary.json`测试统计。

本地产物与完整日志保存在主项目 `review/release-qiwolf8-20261023/`：`artifacts/build-test/`为本地构建，`public/assets/`为实际公开下载；两组产物分别核验，不声称字节相同。失败及重试日志保留于`evidence/build-test/`，公开验收日志在`public/evidence/`。
