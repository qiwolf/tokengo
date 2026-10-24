# qiwolf.14 公开发行收据

正式版：[v0.34.0-qiwolf.14](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.14)；发行源码 `1386fa2d410ece30e1a24009e21fa9a1979cdd01`；2026-10-24 11:59:16 UTC 发布。Cargo0.34.0/schema40；Linux x86_64 原生和 linux/amd64 容器。

[精确 main CI](https://github.com/qiwolf/tokengo/actions/runs/40090424619)、[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40091159611)均成功。build-test 完整工作区 1182 通过、0 失败、3 忽略，另 Python22 项；应用、构建与测试源和最终发行提交逐字一致，仅文档与证据不同。最终 SHA 的 native/WASM、打包、架构/glibc 以及原生/容器生命周期通过；精确 main CI 独立1182/0/3。阶段性和重复覆盖不叠加。忽略项见[本版说明](../../releases/v0.34.0-qiwolf.14.md)。隔离容器/卷和原生临时运行环境已清理，日志、快照及交付产物保留。

Release 正式非 draft、非 prerelease，9 资产。公开原生包 `tokengo-1386fa2d410e-linux-x86_64.tar.gz` SHA256 `0b2cfcaccd9b762a75baff0cf9ab734bc21bb0ea0f17817e2a5166a231426e05`；不可变镜像 `ghcr.io/qiwolf/tokengo@sha256:2cf6dea643149c511785fda0ebc0c399032b2a763c8507eea73430e0b17c5264`。实值来自正式公开资产；已独立匿名下载并匹配官方 digest，不使用本地编译哈希替代。

公开独立验收全部通过：9资产逐项官方 GitHub digest、大小与 SHA256 匹配；实际匿名 pull by digest；版本/完整 SHA/latest 三个注册表标签指向同一 digest；离线双标签导入；无 .env 固定 SHA Compose；非 root/只读根及 ready/WASM；原生/容器 Key 生命周期；重启及同卷重建。隔离容器、卷、原生进程和临时运行目录均 0 残留，临时数据库、主密钥、会话及密码由生命周期脚本清理，下载资产和日志保留。

最后一项 40MB 离线镜像的首轮下载在180秒内未传完，保留失败日志后把独立验收下载时限改为600秒，重新匿名下载成功；8个已完整下载资产先按大小和官方 digest 核对再复用。所有9资产均按原校验标准验收，不计为产品测试失败。[机器可读收据](receipt.json)只含公开产物、验证结果与清理标量，不复制临时凭据或原始运行输入。

本发行阶段未访问或升级生产237。此前私有修复构建的真实共享推理、未定价账本与临时配置清理见[独立生产收据](../../maintenance/shared-deepseek-live-20261024/README.md)。本收据不将真实生产调用表述为公开发行包升级验收。标签及资产文档为构建快照，后补收据维护于 main/Release 页面，不重打资产或移动标签。
