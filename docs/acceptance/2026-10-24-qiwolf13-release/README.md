# qiwolf.13 公开发行验收

正式版：[v0.34.0-qiwolf.13](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.13)，发行源码 `825285e12997ae1e07e1e4e4dce6a45b8b3c6793`，发布时间 UTC 2026-10-23T19:29:37Z。Cargo0.34.0/schema40；Linux x86_64原生程序与linux/amd64容器。

[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40008836973)、[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40010269779)均成功。build-test在本次精确提交上完整workspace **1166通过、0失败、3忽略**，另Python **22项**通过；native/WASM release构建、本地原生与容器运行检查退出码均为0。三个忽略项分别为由父测试执行的子进程helper、外部联网registry测试、需要显式真实来源凭据的回退测试；后续生产阶段另行进行真实回退，不把重复运行或不同阶段数量相加。本轮后端有修改，重新执行完整workspace，没有沿用q12后端复用证据。

新增6项回归覆盖effort对象与标准写法等价、同值双字段合并及冲突拒绝、未知属性/畸形值严格拒绝、JSON/SSE实际原生Responses上游effort不被默认覆盖、未声明档位零出站，以及模型/用户/Key/渠道的实时撤权。原生协议、不透明历史与400分类合同保留。

9个公开资产匿名下载校验通过；公开原生包 `tokengo-825285e12997-linux-x86_64.tar.gz` SHA256为 `9711d054054f05c7d92bd9db2dcd26da207d3c891bd538721012b298872e2574`，镜像为 `ghcr.io/qiwolf/tokengo@sha256:a484d133d8fbc0f613bedae3830b70dbbd743988c67d7c9a26a0c7df0774c99c`。匿名registry实际拉取、离线双标签、无.env固定引用Compose、非root与只读根、ready/WASM、重启与同卷重建、原生程序、原生及容器Key恢复/删除功能均通过。这里的哈希来自公开资产，未以本地独立编译的哈希替代。

本收据描述独立发行隔离验收，`production_access=false`、`production_upgrade_performed=false`保持该阶段事实。后续生产升级另列收据，不能从发布成功推定升级完成。tag与下载资产内文档为构建快照，后补收据维护于main和Release页面，不表示重新打包或移动标签。详见[结构化收据](receipt.json)。
