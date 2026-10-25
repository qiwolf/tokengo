# 容器部署与分发

当前正式版 **[v0.34.0-qiwolf.17](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.17)**，发行源码`0aced3ddbdcc`，Cargo0.34.0/schema40。包含Luna与DeepSeek推理关闭的可分发兼容、目录effort解析与原生thinking关闭控制，并保留移动端模型分类/筛选紧凑布局。兼容限定已核实的官方来源和精确型号；冲突拒绝，管理员手工列表及默认值保留，客户端显式参数优先，第三方不自动套用。[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40159567667)和[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40159639375)成功；9个公开资产、匿名GHCR拉取、配套原生/容器生命周期及Key验收通过。build-test完整工作区Rust **1253通过、0失败、3忽略**，Python三组分别13、6、3项，重复覆盖不累加。见[版本说明](https://github.com/qiwolf/tokengo/blob/main/docs/releases/v0.34.0-qiwolf.17.md)与[发布记录](https://github.com/qiwolf/tokengo/blob/main/docs/acceptance/2026-10-25-qiwolf17-release/README.md)。标签及包内文档为构建快照，后补收据维护于main与Release页面，不移动已发布标签。

历史公开版 **[v0.34.0-qiwolf.15](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.15)**，发行源码`c3ecd31250f0`，Cargo0.34.0/schema40。统一控制台、系统设置分组和关键弹窗，修正表单、聚焦与错误状态样式；没有业务接口或数据库迁移变更。本地合并后的原生/WASM构建及原生/容器测试通过；精确main CI和发行流程已成功，9个附件与GHCR镜像已公开。已验收UI完成284项有效浏览器检查。标签/包内文档为构建快照，后补收据维护于main和Release页面。见[版本说明](releases/v0.34.0-qiwolf.15.md)与[发布记录](acceptance/2026-10-25-qiwolf15-release/README.md)。

历史公开版 **[v0.34.0-qiwolf.14](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.14)**，发行源码`1386fa2d410e`，Cargo0.34.0/schema40。包含账务管理、模型映射移除与重加、共享候选与执行一致性修复；main CI及发布工作流成功，9个公开资产已发布。build-test完整Rust1182通过、0失败、3忽略，另Python22项；最终发行提交的native/WASM及原生/容器隔离运行已通过。公开9资产匿名校验、GHCR按digest实际匿名拉取、固定SHA Compose及原生/容器独立运行全部通过，见[版本说明](releases/v0.34.0-qiwolf.14.md)和[发行收据](acceptance/2026-10-24-qiwolf14-release/README.md)。237的真实推理与未定价账本验收属于此前私有修复构建，见[生产收据](maintenance/shared-deepseek-live-20261024/README.md)；本次公开发行没有再次升级237。标签/下载包文档为构建快照，后补收据维护于main和Release页面。

镜像包含同一源码提交的 Rust 服务端、CLI 和完整 WASM 控制台，不需要在运行主机安装 Rust、Node.js 或 Nginx。默认源码多阶段构建；Linux CI 也将已经校验的同提交原生包封装成镜像，并验证隔离容器的启动、鉴权、WASM、重启持久化及停止行为。

本仓库同时提供直接部署的 Linux 原生包与容器两种交付方式。已发布版本可在 [GitHub Releases](https://github.com/qiwolf/tokengo/releases) 下载原生包、离线镜像及校验和。从 `v0.34.0-qiwolf.4` 起，fork 版本标签经 CI 验证后还推送至 `ghcr.io/qiwolf/tokengo`；普通 PR 和手动构建只生成审阅制品，不发布。Actions 制品保留14天，长期分发以 Releases 附件和 GHCR 为准。当前发布架构为 **Linux AMD64**；ARM64 尚未验收。

部署后的智能体管理入口见[运维令牌](operations-tokens.md)（自 v0.34.0-qiwolf.5 提供，须核对部署版本）；较早版本可参考[root 运维会话兼容说明](account-login.md#脚本化运维)。

历史公开分发 **[v0.34.0-qiwolf.9](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.9)**，Cargo0.34.0/schema40。发行源码 `8598fc070ca4a91e700d5748947d0343766ad06b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39898866368)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39898890902)均通过。9个公开资产匿名下载校验、GHCR匿名按摘要拉取、原生与容器Key运行检查及无.env固定SHA Compose启动/重启/同卷重建通过。原生包SHA256 `8ef62e86b8f845add9d1404432980afb8776b7071ada89f39a740c11b26cea3a`；固定镜像 `ghcr.io/qiwolf/tokengo@sha256:a25ef92a226b8921e3339d744e718572f10fd7bf7e7b4258e1e8c77d7aa5250a`。详见[版本收据](releases/v0.34.0-qiwolf.9.md)与[公开产物验收](acceptance/2026-10-23-qiwolf9-release/README.md)。标签及公开资产内的文档是发布前源码快照；完整发布后收据保存在main分支与GitHub Release说明中，不代表先前资产已包含后补回执。

Key恢复材料复用现有AES-256-GCM密文字段；升级备份必须同时保存数据库与原主密钥。只有摘要的历史Key不能补算出原值，不会自动替换、撤销或扩大现有Key授权。旧版本不能提供新版的列表删除保护与再次显示合同；即使schema仍为40，回滚也应恢复匹配的程序、Web、数据库与主密钥快照，并先处理升级后的业务写入，不能只凭schema号相同判断可安全降级。

历史公开分发 **[v0.34.0-qiwolf.8](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.8)**，功能schema40；新增功能、现场验证与公开发布收据见[版本说明](releases/v0.34.0-qiwolf.8.md)。发行源码 `ca008c7acfcc53d49c5c59670dd123fc28c37a5b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39835960738)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39837203892)通过。Linux x86_64 原生包与 linux/amd64 容器已公开，9个资产匿名下载校验、GHCR按摘要匿名拉取、无.env的Compose启动以及原生/容器实际运行和重启/同卷重建通过。上一公开版 [v0.34.0-qiwolf.7](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.7) 对应源码 `684fa862c64bab5b16c438b7fb8db03295c5b8f4`、schema39，修复固定SHA Compose与离线双标签；本轮保留该机制。升级schema40前一致备份数据卷、主密钥与旧镜像，回滚使用配套旧数据库。安装不自动切换既有调用方；MinerU原生异步资源API仍不支持，见[透传文档](passthrough.md)。

## 从镜像仓库安装

从同一 Release 下载 `compose.yaml`、`container.env.example`。qiwolf.7 起的发布配置直接写入固定提交的 `image: ghcr.io/qiwolf/tokengo:sha-<完整SHA>`，不包含源码 `build`；在 1Panel 等面板中只导入该 Compose 即可确定镜像，不依赖隐藏的 `.env`。源码仓库根目录的 Compose 仍用于源码构建，不能与发布配置混淆。

已发布的 qiwolf.6 及更早附件使用源码模板 `image: ${TOKENGO_IMAGE:-tokengo:local}`，没有配套 `.env` 时会退回本地构建标签；这不表示 GHCR 镜像没有版本标签。在面板中使用这些旧附件时，将 `image:` 行直接改为下列明确版本，并移除 `build:` 配置；命令行也可复制 `container.env.example` 为 `.env` 并修改其中 `TOKENGO_IMAGE`：

```yaml
services:
  tokengo:
    image: ghcr.io/qiwolf/tokengo:v0.34.0-qiwolf.6
```

保留原配置中其余端口、数据卷及安全设置；使用旧变量模板的命令行部署则在 `.env` 中设置 `TOKENGO_IMAGE=ghcr.io/qiwolf/tokengo:v0.34.0-qiwolf.6`。

然后在包含这两个配置文件的目录执行：

```sh
docker compose pull
docker compose up -d --no-build
docker compose ps
curl --fail http://127.0.0.1:3160/readyz
```

Release 的 `registry-image.txt` 与发布说明记录镜像摘要；需要固定不可变镜像时，修改发布 Compose 的 `image:` 行（旧变量模板则修改 `.env` 中的 `TOKENGO_IMAGE`），使用其中完整的 `ghcr.io/qiwolf/tokengo@sha256:...`。历史qiwolf.7已核对的不可变镜像为 `ghcr.io/qiwolf/tokengo@sha256:92f3b51bf235098d1d68ab76319bd3c0832c7bebb092ceae97d25312339b931a`；qiwolf.8已验收镜像为 `ghcr.io/qiwolf/tokengo@sha256:7f8517a515c80d25635d0093f28fe3159bc4bd39b1e946c8e147650a815fe3d4`，不沿用历史摘要。同时提供 `sha-<完整提交>` 和 `latest` 标签；正式部署建议固定版本或摘要。镜像摘要可识别内容，不替代下载来源与源码提交核对。

## 从源码构建并首次启动

安装 Docker Engine / Docker Desktop 与 Compose v2 或更新版，获取本仓库并固定已审阅的提交。以下命令在仓库根目录执行：

```sh
cp deploy/container.env.example .env
# 将 .env 的 TOKENGO_REVISION 设置为 git rev-parse HEAD 输出的完整提交号。
docker compose build --pull
docker compose up -d
docker compose ps
curl --fail http://127.0.0.1:3160/readyz
```

访问 `http://127.0.0.1:3160/ui/`，在首次设置页自行设置管理员密码，保存页面生成的 Gateway Key。初始管理员登录名是 `admin`，随后可在“我的账号”修改。镜像不附带默认密码、供应商密钥或生产数据库。首次设置前保持默认回环绑定；远程服务器可先用 SSH 转发：

```sh
ssh -L 3160:127.0.0.1:3160 your-server
```

对外提供服务时，在自己的 HTTPS 反向代理后开放入口；将 `.env` 的 `TOKENGO_BIND` 改成目标地址前完成首次设置。Web/API 使用同源入口，SSE 应关闭代理缓冲并设置足够的流式超时。健康检查读取 `/readyz` 的 JSON 状态，HTTP 200 本身不能代表数据库就绪。

Compose 默认使用命名卷 `tokengo-data`（实际名称含 Compose 项目前缀）。固定同一个项目目录或使用 `docker compose -p tokengo ...`，避免换目录后误创建一个全新的空卷。容器以 UID/GID `10001:10001` 运行，根文件系统只读，删除 Linux capabilities；仅 `/data` 与临时 `/tmp` 可写。

## 数据与主密钥

`/data` 包含数据库、WAL/SHM、结算 outbox、主密钥和轮转日志：

| 路径 | 用途 |
| --- | --- |
| `/data/tokengo.db`、`-wal`、`-shm` | 用户、来源、价格、账务及运行状态 |
| `/data/tokengo.db.billing-outbox/` | 已持久化的结算意图（如存在） |
| `/data/master.key` | 首次启动生成，0600；解密已保存来源凭据必需 |
| `/data/logs/` | 有上限的轮转日志 |

不要把数据库或 master.key 放入镜像、`.env` 或 Git。丢失主密钥后原来源凭据无法解密。挂载自有目录时，先将目录属主设为 `10001:10001`、权限0700，已有 master.key 保持0600；不要用 `chmod 777` 解决权限。默认命名卷会保留镜像预置的 `/data` 属主。

备份或迁移先停止入口新流量，等运行中请求结束及账务补写完成，再停止服务，使用能保留权限的备份工具保存**完整卷**，单独保管主密钥。服务停止后再复制 SQLite 及 outbox，不能只在线复制一个 `.db` 文件。容器重建、`docker compose down` 保留命名卷；`docker compose down -v` 会删除数据卷，不用于常规升级。

## 离线镜像分发

从 GitHub Release 下载对应版本的镜像、`image-SHA256SUMS`、`compose.yaml`、`container.env.example`、`container-deployment.md` 和 `image-manifest.json`；也可下载对应 GitHub Actions Linux release package 任务的 `tokengo-image-linux-amd64-<完整SHA>` artifact。将这些文件放在同一目录，在目标主机执行：

```sh
sha256sum --check image-SHA256SUMS
docker load -i tokengo-<完整SHA>-linux-amd64.tar.gz
```

qiwolf.7 起的离线镜像同时保留 `tokengo:sha-<完整SHA>` 与发布 Compose 使用的 `ghcr.io/qiwolf/tokengo:sha-<完整SHA>` 两个标签；`docker load` 后可直接使用配套 Compose，无需拉取或源码构建。qiwolf.6 及更早离线包只带前者，需将 `image:` 直接设为 `tokengo:sha-<完整SHA>` 并移除 `build:`，或复制示例为 `.env` 并设置同一 `TOKENGO_IMAGE`。然后：

```sh
docker compose up -d --no-build
docker compose ps
```

初次安装生成独立数据卷，不带分享者的用户或上游密钥。离线分发前可检查镜像来源标签：

```sh
docker image inspect tokengo:sha-<完整SHA> --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}'
```

也可以自行构建后分发，镜像 tag 与来源提交必须相符：

```sh
docker build --build-arg VCS_REF="$(git rev-parse HEAD)" -t tokengo:my-reviewed-build .
docker save tokengo:my-reviewed-build | gzip > tokengo-image.tar.gz
sha256sum tokengo-image.tar.gz > tokengo-image.tar.gz.sha256
```

源码构建依赖 Docker Hub、Debian 软件源、crates.io，以及 Trunk 获取匹配的 WASM 工具；首次构建可能较慢。不要使用生产数据库作为 build context。`.dockerignore` 默认拒绝全部文件，仅放行源码、构建清单、许可证与专用容器辅助脚本。

## 升级、停止与回滚

先备份完整数据卷与主密钥，再导入新镜像，将发布 Compose 的 `image:` 行指向明确 SHA tag（旧变量模板修改 `.env` 中的 `TOKENGO_IMAGE`），执行 `docker compose up -d --no-build`。核对 `/readyz`、控制台登录、来源/模型目录及一笔受控真实调用，必要时核对财务账本；容器健康不等于所有上游模型均可用。

服务端收到 SIGTERM/SIGINT 后停止接受新请求，默认最多等120秒请求结束，再给账务队列最多10秒排空；Compose 为整个停止过程预留150秒。`TOKENGO_SHUTDOWN_TIMEOUT_SECS` 可调整请求排空时间，同时须增加 `stop_grace_period` 以容纳账务与运行时退出。正常操作先排空流量再停止，避免长流式任务跨越截止时间；超时后 Docker 会强制结束进程。账务恢复依靠原有 FULL / outbox 合同，不能保证任意强制杀进程后全部非持久运行日志仍在。

回滚使用旧镜像配套的停机前备份恢复数据卷和主密钥。数据库已迁移后，不保证旧程序能读取新 schema；不要直接降级镜像并继续写同一新数据库。单实例 SQLite 不支持多个容器同时共享同一卷来扩容。

## 开发者验证

```sh
docker compose config --quiet
sh -n deploy/container/healthcheck.sh
docker build --check .
python3 deploy/container/smoke.py tokengo:local
```

Smoke 使用独立随机命名的容器与卷，临时密码只在内存中生成，结束删除测试卷。它验证本地镜像的无上游启动、实际 WASM 资产、未登录管理鉴权、首次设置、非 root、主密钥0600、数据库/密钥重启持久化与正常停止；不访问真实供应商，不替代路由或财务回归测试。
