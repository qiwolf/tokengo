# v0.34.0-qiwolf.33 发行收据

状态：已于2026-11-14 12:47:08 UTC正式发行，公开产物独立验收通过。精确源码 `390c9ad52e17e95015062b06bf990b02d48421fb`，程序版本 `0.34.0-qiwolf.33`，schema47。

## build-test

Mac 管理源码及 Git，完整提交经 Git bundle 同步至独立目录 `/srv/build/tokengo-qiwolf33-20261114/source`。逐文件 SHA256 留存，真实环境和凭据没有进入源码或测试。

原生 server/CLI 与 release WASM 构建通过，发行包校验及隔离运行通过；CLI 首行、服务端 auth/status 和真实 WASM 页脚均核对为 `0.34.0-qiwolf.33`。Chromium/WebKit ×390/1440 ×普通用户/管理员共8场景76项通过，含独立入口、直达/历史、统计失败隔离、完整合成 Key 生命周期与角色边界。原生隔离测试验证 schema47、迁移独立运行、Key加密与重启认证持久化。没有操作生产用户 Key。

初轮原生和WASM已编译成功，打包因脚本未识别外置缓存目录失败；保留日志后将构建副本 target 链接到实际缓存并重新打包成功。没有修改产品代码或忽略检查。

功能修复的早期226项 Web 单测与3个延迟响应回归来自 `12cd8166da36cccc086f68b0250c410b9dc21a8c`，见[专项收据](../../maintenance/personal-key-entry-20261114/README.md)，与本版新测分开，重叠不累加。完整日志/包已回收 Mac 项目 `review/release-qiwolf33-20261114/`。

## 正式发行与公开验收

[精确 main CI](https://github.com/qiwolf/tokengo/actions/runs/43594017528)第二次运行通过 Rust1779/0/15、Python13、WASM与5项显式历史规模检查（重叠不累加）；[发行工作流](https://github.com/qiwolf/tokengo/actions/runs/43594731261)全部成功，[GitHub正式版](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.33)公开9项附件及GHCR镜像。首轮 CI 的 `proxy::nonstream_body_timeout_is_not_an_empty_success` 在 HTTP 成功后单次 drain 读到延后落库的 running 状态，原日志保留。fixture 已说明 drain 是可延后而非完成屏障；同源码 build-test 该项连续20轮通过，GitHub 仅失败作业已重跑，未改断言/忽略测试。最终完整回归已通过。

## 范围

本版没有新增真实模型调用验收，不把既有线上修复当成本次发行部署。实体 iPhone 未测试。旧 .32 标签、资产保持不变；本版包内文档保留提交时快照，最终状态维护于 main 和 Release 正文。

## 公开产物验收结果

九项附件经 Mac 匿名下载，与 GitHub 官方大小及 SHA256 digest 全部一致，再同步至 build-test；原生包内828份文档逐份对照标签源码一致，standalone部署文档副本、manifest、Cargo.lock、迁移及x86_64 ELF身份均通过。原生 CLI/服务端精确版本、schema47、Key加密和重启认证通过。

匿名 GHCR 拉取重试通过，digest 为 `sha256:8e3d9384af34b6c9376117187cfc1dc35cbe7c081eb4e0bb9cb54f815e3d0894`，与离线镜像及源码标签一致。离线加载、容器新库初始化/Key认证/重启/重建持久化通过；Compose仅检查配置。公开原生包内真实 WASM 再跑8场景76项浏览器检查通过，含精确版本号，未使用生产凭据。

[机器可读收据](evidence/summary.json)保留官方九项digest和CI结果。build-test下载/GHCR首次TLS中断、私用验收续跑脚本变量缺失的中断日志均保留；前段已通过收据复用，后段容器/公开Web补跑成功，不把中断记为通过。所有日志和交付包已取回 Mac `review/release-qiwolf33-20261114/`。本次正式发行没有再次部署生产实例；既有Key入口线上修复仍按专项上线记录追踪。
