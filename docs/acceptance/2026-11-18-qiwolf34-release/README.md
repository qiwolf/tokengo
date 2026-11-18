# v0.34.0-qiwolf.34 发行收据

[v0.34.0-qiwolf.34](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.34) 于 2026-11-18 15:15:46 UTC 正式公开。标签对应 d9c423a10fa858a5ec71dafb98a8c8bce892f210，程序版本为 0.34.0-qiwolf.34，数据库 schema 为 50。功能范围见[版本说明](../../releases/v0.34.0-qiwolf.34.md)。

## 构建与公开产物

- build-test 的锁文件、工作区版本和 schema 核对通过，部署与容器脚本共 13 项测试通过。
- [精确 main CI](https://github.com/qiwolf/tokengo/actions/runs/44328046577) 通过 Rust 1,782 项、Python 13 项及 WebAssembly 检查。Rust 失败 0 项、忽略 15 项。大历史数据专项另通过 5 项。
- [发行工作流](https://github.com/qiwolf/tokengo/actions/runs/44330789518) 构建并验证同提交的 Linux x86‑64 服务端、CLI、WebAssembly 和 Linux amd64 容器，发布九项附件。匿名下载后的长度和 SHA‑256 与 GitHub 官方值一致。原生包中 838 份文档逐份匹配标签源码，迁移 SQL 和包内校验清单一致。
- build-test 独立运行公开原生包，核对了 CLI、服务端版本、新库 schema 50 和重启后的 Key 认证。从旧版公开 .33 包建立的库实际升级到 schema 50，保留合成的人民币及美元历史价格、原数据库文件身份与主密钥。重复执行离线迁移结果一致。
- 从公开 Linux 包提取 WebAssembly，桌面和窄屏分别通过 12 项 Chromium 检查，覆盖版本显示、原生 Responses 设置、保存和同上游映射弹窗。控制台验证使用隔离 API。离线容器通过新库初始化、Key 认证、重启和重建持久化检查。Compose 验证配置解析。

- 容器镜像为 ghcr.io/qiwolf/tokengo@sha256:6c502d26cdd321341107ad99fd350d4592ab6e1da29d2489609ec784cbe5b077。匿名 OCI 接口读回版本标签、源码标签和摘要引用一致，镜像配置与离线包一致，8 层全部通过长度及 SHA‑256 核对。build-test 的 Docker 直拉在取令牌时遇到 EOF。改用匿名 OCI 接口，经 Mac 下载并在 build-test 复核。

[机器可读收据](evidence/summary.json)记录公开资产摘要和受测范围。

## 验收范围

功能源码此前已在 237 核验模型规格目录、渠道覆盖、定价、映射、Responses 和 DSH 网页工具调用。本次公开包验证在隔离环境中完成，237 生产验收仍对应[原部署收据](../2026-11-18-model-specifications-deploy237/README.md)。

最初的升级验收夹具把离线迁移视为创建主密钥的入口。改用旧版公开二进制实际启动建立主密钥后，重新执行通过。正式附件始终使用 GitHub 原发布内容。正式标签下的打包快照保留发布前文档，最终发行收据记录在 main。
