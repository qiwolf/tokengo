# 容器部署与分发

镜像包含同一源码提交的 Rust 服务端、CLI 和完整 WASM 控制台，不需要在运行主机安装 Rust、Node.js 或 Nginx。默认源码多阶段构建；Linux CI 也将已经校验的同提交原生包封装成镜像，并验证隔离容器的启动、鉴权、WASM、重启持久化及停止行为。

本仓库同时提供直接部署的 Linux 原生包与容器两种交付方式。已发布版本可在 [GitHub Releases](https://github.com/qiwolf/tokengo/releases) 下载原生包、离线镜像及校验和。从 `v0.34.0-qiwolf.4` 起，fork 版本标签经 CI 验证后还推送至 `ghcr.io/qiwolf/tokengo`；普通 PR 和手动构建只生成审阅制品，不发布。Actions 制品保留14天，长期分发以 Releases 附件和 GHCR 为准。当前发布架构为 **Linux AMD64**；ARM64 尚未验收。

部署后的智能体管理入口见[运维令牌](https://github.com/qiwolf/tokengo/blob/v0.34.0-qiwolf.6/docs/operations-tokens.md)（自 v0.34.0-qiwolf.5 提供，须核对部署版本）；较早版本可参考[root 运维会话兼容说明](https://github.com/qiwolf/tokengo/blob/v0.34.0-qiwolf.6/docs/account-login.md#脚本化运维)。

当前正式版本为 [v0.34.0-qiwolf.6](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.6)，对应源码 `d5660992a6a1ef0b320ff143a19404ac3abad23e`、schema39。新增通用 HTTP 透传和 MinerU 同步接入；配置及限制见[透传文档](https://github.com/qiwolf/tokengo/blob/v0.34.0-qiwolf.6/docs/passthrough.md)。不自动切换既有调用方，也不提供 MinerU 原生异步资源 API。

## 从镜像仓库安装

从同一 Release 下载 `compose.yaml`、`container.env.example`，复制后者为 `.env`，设置明确版本：

```sh
TOKENGO_IMAGE=ghcr.io/qiwolf/tokengo:v0.34.0-qiwolf.6
```

然后在包含这两个配置文件的目录执行：

```sh
docker compose pull
docker compose up -d --no-build
docker compose ps
curl --fail http://127.0.0.1:3160/readyz
```

Release 的 `registry-image.txt` 与发布说明记录镜像摘要；需要固定不可变镜像时，将 `TOKENGO_IMAGE` 改为其中完整的 `ghcr.io/qiwolf/tokengo@sha256:...`。本版已核对的不可变镜像为 `ghcr.io/qiwolf/tokengo@sha256:13b327a4d0c308fbdb7734d4e0550f58589aac1bafed57775cb06cd74f64034f`。同时提供 `sha-<完整提交>` 和 `latest` 标签；正式部署建议固定版本或摘要。镜像摘要可识别内容，不替代下载来源与源码提交核对。

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

将 `compose.yaml` 与 `container.env.example` 一并交给使用者。复制示例为 `.env`，设置 `TOKENGO_IMAGE=tokengo:sha-<完整SHA>`，然后：

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

先备份完整数据卷与主密钥，再导入新镜像，将 `.env` 指向明确 SHA tag，执行 `docker compose up -d --no-build`。核对 `/readyz`、控制台登录、来源/模型目录及一笔受控真实调用，必要时核对财务账本；容器健康不等于所有上游模型均可用。

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
