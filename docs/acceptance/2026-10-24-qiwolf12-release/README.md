# qiwolf.12 公开发行验收

**[v0.34.0-qiwolf.12](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.12) 已发布并通过公开产物独立验收。** 发布时间2026-10-23 18:25:37 UTC（北京时间2026-10-24 02:25:37）；发行源码 `6bf36fa917cd87209286c88a805ddc2cb6e52669`，Cargo0.34.0/schema40。发布目标为Linux x86_64原生及linux/amd64容器。

[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40000977772)与[发布工作流](https://github.com/qiwolf/tokengo/actions/runs/40001310546)均成功。GitHub对精确发行提交运行完整Rust工作区，**1160通过、0失败、3忽略**。忽略项分别为由父测试调用的子进程helper、外部联网registry测试、需显式凭据的真实云到GPU向量/重排回退测试；不将三项统称为凭据缺失。本轮build-test重新执行**Web125项、Python22项**并通过，native/WASM release构建与本地原生/容器Key及生命周期检查通过。172个后端文件与此前0d0e6e7完整工作区检查的源码逐字节一致，后端沿用该证据；**没有在本轮build-test重新运行完整工作区**，也不将CI、旧检查和重复运行相加。

9个公开资产匿名下载校验通过。公开原生包 `tokengo-6bf36fa917cd-linux-x86_64.tar.gz` 的SHA256为 `aa4b94472ffecf6a5a941f2c05e5e7eb55ccd7b380da4409f2dfe06271eb3393`；GHCR不可变镜像为 `ghcr.io/qiwolf/tokengo@sha256:ac134402e1f4f3af17aa321b63a9e29235f41b597ef8f007dc822c2e6d93c583`。匿名实际拉取、离线镜像双标签、无.env固定引用Compose启动、非root/只读根、ready/WASM、重启/同卷重建、原生运行与原生/容器Key检查均通过。公开产物身份与本地独立构建hash分开，不混用。

本轮没有访问或升级生产237，现场仍运行私有 `0d0e6e7` 性能补丁，不视为已切换qiwolf.12正式产物。GPU分词器及8192窗口为独立现场配置，网关包不会自动修改。标签和资产内文档是构建时快照；最终发布后收据维护在main和Release页面，后补文档不表示资产重新打包。

[结构化收据](receipt.json)记录本轮实际范围。私人语料、Key和数据库不进入公开收据；此前现场真实推理和私有性能验收与本次公开包运行检查分别保留，不互相冒充。
