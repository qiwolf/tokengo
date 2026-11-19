# v0.34.0-qiwolf.35 发行收据

[v0.34.0-qiwolf.35](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.35) 于北京时间 2026-11-20 01:46:53 正式公开。标签指向 `69dce817c1322a2fff8411ea249394fb1e2cc52e`。

程序版本为 `0.34.0-qiwolf.35`，数据库 schema 为 50。功能范围见[版本说明](../../releases/v0.34.0-qiwolf.35.md)。

- 用量折叠提交 `6a2a19b` 已在 237 通过实测。同一真实账目区间内，右侧 9 项默认显示 4 项，图表说明到卡片底边由 280px 缩至 17px。[生产收据](../2026-11-19-usage-model-collapse-deploy237/README.md)
- Key 模型候选筛选提交 `4172f82` 已通过 build-test 的 225 项前端单测、WASM 构建和 Chromium 合成账户回归，覆盖授权保存前后及新建和编辑页面。237 使用现有用户真实授权验证新建 Key 的候选从 31 个收至 18 个，未授权候选为 0。[生产收据](../2026-11-20-key-user-model-scope-deploy237/README.md)

## CI 与公开制品

- 同一提交的 [main CI](https://github.com/qiwolf/tokengo/actions/runs/44578261603) 和[发行工作流](https://github.com/qiwolf/tokengo/actions/runs/44578395371) 均成功。主分支 CI 记录 Rust 1,784 项通过、15 项既有忽略，Python 13 项通过，大历史数据专项另通过 5 项。发行工作流验证了原生包、离线容器及容器重启和重建后的持久化，再发布 GitHub Release 和 GHCR 镜像。
- 独立下载 GitHub 的 9 项公开附件并核对长度和 SHA-256。原生包的 845 份文档、打包校验清单和迁移 SQL 与标签源码核对一致。build-test 实际运行原生包，验证 CLI、服务端版本、新库 schema 50 及重启后的 Key 认证。
- 从公开原生包提取 WASM，在 build-test Chromium 复核了用量折叠和管理员及个人空间的 Key 模型候选。浏览器使用合成账户 API。
- 匿名 OCI 回读核对了版本标签、源码标签、摘要和镜像配置，另核对了 8 个镜像层的长度和 SHA-256。镜像为 `ghcr.io/qiwolf/tokengo@sha256:4c7c943f69ec29fa238be15ef257c2746f9b6e02eb765340fee5a4d5fd7525af`。

237 实际运行本次两个修复的 Web 源码，生产服务端仍为 `a6e3e85`。公开原生包的新库验收在隔离环境完成，旧库升级延续 .34 的专项证据。标签保留发版时打包的文档快照，main 提交记录实际发行回执。

[机器可读发行收据](receipt.json)
