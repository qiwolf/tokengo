# Linux x86_64 私有站部署与回滚

当前正式版 **[v0.34.0-qiwolf.13](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.13)**，发行源码`825285e12997`，Cargo0.34.0/schema40。修复Chat→Responses拒绝effort对象的兼容缺口，保留显式档位、来源能力校验与实时授权。本轮完整Rust1166/Python22项、native/WASM及9个公开资产和原生/容器验收通过。237已冷备升级；既有Key的普通/流式推理、原生Responses、向量/重排回退、授权与Key功能通过。见[版本说明](releases/v0.34.0-qiwolf.13.md)、[公开发行收据](acceptance/2026-10-24-qiwolf13-release/README.md)与[独立升级收据](acceptance/2026-10-24-qiwolf13-upgrade237/README.md)。标签/下载资产文档为构建快照，后补收据维护在main和Release页面。

这套交付先在新 Linux LXC 上独立验收，默认仅监听 `127.0.0.1:3160`。它不修改 LiteLLM、反向代理、DNS 或现有客户端入口。真实模型逐项验收和最终切流另行执行；通过 `/readyz` 不等于工具调用或模型迁移已通过。

GitHub Releases 提供 Linux x86_64 直接部署包及容器版，选择其中一种安装方式。直接包包含服务端、CLI、完整 WASM 控制台、安装/升级/回滚工具、匹配源码的完整 `docs/` 和可选的 `tools/mineru-gateway-client.py`；Rust 服务运行不依赖该 Python 客户端。容器部署见[容器指南](container-deployment.md)。各版本以实际发布附件、精确提交 CI 和目标机验收记录为准。不得把 macOS 二进制放入 Linux 交付包。

部署后的智能体管理入口见[运维令牌](operations-tokens.md)（自 v0.34.0-qiwolf.5 提供，须核对部署版本）；较早版本可参考[root 运维会话兼容说明](account-login.md#脚本化运维)。

历史公开分发 **[v0.34.0-qiwolf.9](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.9)**，Cargo0.34.0/schema40。发行源码 `8598fc070ca4a91e700d5748947d0343766ad06b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39898866368)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39898890902)均通过。9个公开资产匿名下载校验、GHCR匿名按摘要拉取、原生与容器Key运行检查及无.env固定SHA Compose启动/重启/同卷重建通过。原生包SHA256 `8ef62e86b8f845add9d1404432980afb8776b7071ada89f39a740c11b26cea3a`；固定镜像 `ghcr.io/qiwolf/tokengo@sha256:a25ef92a226b8921e3339d744e718572f10fd7bf7e7b4258e1e8c77d7aa5250a`。详见[版本收据](releases/v0.34.0-qiwolf.9.md)与[公开产物验收](acceptance/2026-10-23-qiwolf9-release/README.md)。标签及公开资产内的文档是发布前源码快照；完整发布后收据保存在main分支与GitHub Release说明中，不代表先前资产已包含后补回执。

Key恢复材料复用现有AES-256-GCM密文字段；升级备份必须同时保存数据库与原主密钥。只有摘要的历史Key不能补算出原值，不会自动替换、撤销或扩大现有Key授权。旧版本不能提供新版的列表删除保护与再次显示合同；即使schema仍为40，回滚也应恢复匹配的程序、Web、数据库与主密钥快照，并先处理升级后的业务写入，不能只凭schema号相同判断可安全降级。

历史公开分发 **[v0.34.0-qiwolf.8](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.8)**，功能schema40，新增共享向量/重排回退、逐来源续验、回收站与服务授权；发行源码 `ca008c7acfcc53d49c5c59670dd123fc28c37a5b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39835960738)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39837203892)通过。Linux x86_64 原生包与 linux/amd64 容器已公开，9个资产匿名下载校验、GHCR按摘要匿名拉取、无.env的Compose启动以及原生/容器实际运行和重启/同卷重建通过。见[本版说明](releases/v0.34.0-qiwolf.8.md)和[验收状态](acceptance-status.md)。上一公开版 [v0.34.0-qiwolf.7](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.7) 对应 `684fa862c64bab5b16c438b7fb8db03295c5b8f4`、schema39，修复容器交付配置。安装包不会自动创建私有路由或切换调用方；[通用透传与MinerU](passthrough-development.md)仍限已实现的同步链路。由schema39升级前须一致备份数据库、主密钥和旧程序，不能用旧程序直接打开schema40数据库。

## 使用已发布的直接部署包

从 [GitHub Releases](https://github.com/qiwolf/tokengo/releases) 下载同一版本的 `tokengo-<提交前12位>-linux-x86_64.tar.gz` 及其 `.sha256` 文件，在独立 Linux x86_64 主机执行（以实际文件名替换占位符）：

```bash
sha256sum --check tokengo-COMMIT-linux-x86_64.tar.gz.sha256
tar -xzf tokengo-COMMIT-linux-x86_64.tar.gz
cd tokengo-COMMIT-linux-x86_64
python3 deploy/manage.py verify ../tokengo-COMMIT-linux-x86_64.tar.gz
sudo python3 deploy/manage.py install ../tokengo-COMMIT-linux-x86_64.tar.gz
sudo python3 /opt/tokengo/current/deploy/manage.py health
```

首次通过下文 SSH 隧道访问控制台完成账号设置，无需安装 Rust 或在目标机编译。目标机需具备 systemd、Python3、CA 证书及兼容 glibc（CI Ubuntu 24.04 基线为2.39；Debian13亦已单独部署验证）。已存在安装时使用后文 `upgrade`，不要重新初始化或覆盖主密钥。

## 构建同一提交的服务端与前端

构建机使用 Ubuntu 24.04 x86_64（glibc 2.39 基线）、Rust 1.98.0、WASM target 和 Trunk 0.21.14。通用 CI 产物以此为兼容基线；实际目标 Debian 13 x86_64 LXC 已有独立部署路径；公开原生包按精确发布提交的 Ubuntu CI 产物核验；此前已有目标机运行证据不代替本轮新产物验收。其他系统须验证 glibc 和架构兼容，不能直接假定可运行。SQLite 使用项目 bundled 模式，构建机仍需 C 编译器。

```bash
sudo apt-get update
sudo apt-get install --no-install-recommends -y build-essential pkg-config libssl-dev python3 ca-certificates
rustup toolchain install 1.98.0 --profile minimal --target wasm32-unknown-unknown,x86_64-unknown-linux-gnu
cargo +1.98.0 install --locked --version 0.21.14 trunk
# 从已提交且干净的 checkout 执行；输出放仓库外。
bash deploy/build-release.sh /tmp/tokengo-release
cd /tmp/tokengo-release
sha256sum --check ./*.sha256
python3 /path/to/checkout/deploy/manage.py verify ./*.tar.gz
```

`release-check.yml` 在同一 checkout 执行 native workspace release 和 `trunk build --release --locked`，验证包内 ELF64 x86_64、前端 index/JS/WASM 以及全部文件校验和，再保存 14 天的 Actions artifact。测试 CI 保持独立；PR 和手动构建只留制品。`qiwolf/tokengo` 的 `v*-qiwolf.*` 标签在打包和容器隔离验收成功后，将同一镜像推送 GHCR、按摘要拉取并再次烟测，然后创建带双版本附件的 GitHub Release。发布流程不连接生产服务器，也不注入生产密钥。Trunk 的固定版本与 `--locked` 来源可核对[官方版本清单](https://github.com/trunk-rs/trunk/blob/v0.21.14/Cargo.toml)和[官方构建配置](https://github.com/trunk-rs/trunk/blob/v0.21.14/src/config/models/build.rs)。Trunk 首次可能下载与 Rust wasm-bindgen 版本匹配的工具及 wasm-opt，构建机需具备相应出网条件。

包内 `manifest.json` 记录完整 source commit、target、Rust/Trunk 版本和 Cargo.lock SHA256；`checksums.json` 覆盖全部交付文件。校验和用于发现传输/文件损坏，不是签名或来源认证：从可信 Actions run 获取，并核对提交 SHA，不能信任来历不明的压缩包及其随附校验和。PR artifact 只用于审阅。

## 新 LXC 安装

准备独立 Linux x86_64 LXC（本次为 Debian 13），给予适量 CPU/内存、持久磁盘、systemd、python3、CA 证书以及受控的上游出网。先打通经授权的 SSH 入口并核对目标机，再复制已验证的包和当前提交的 `deploy/manage.py`；不要为此修改现有 LiteLLM 容器。

```bash
# 下面均在新 LXC，本地管理端口通过 SSH 隧道访问。
python3 deploy/manage.py verify /tmp/tokengo-COMMIT-linux-x86_64.tar.gz
sudo python3 deploy/manage.py install /tmp/tokengo-COMMIT-linux-x86_64.tar.gz
sudo systemctl status tokengo --no-pager
sudo python3 /opt/tokengo/current/deploy/manage.py health
# 在管理电脑执行（HOST 替换为经核实的新 LXC）：
ssh -N -L 3160:127.0.0.1:3160 USER@HOST
```

访问 `http://127.0.0.1:3160/ui/` 完成首次面板密码与 VKey 设置。密钥只通过授权的管理流程输入，避免命令行参数、工单或 shell 日志。新站初始为空，不会自动导入 LiteLLM 密钥、用户、路由或价格。管理界面已提供用户/VKey、预算、价格、账本、模型路由和源协议编辑。预算冻结解除等尚未实现的操作以页面明确标注为准。

安装器创建无登录 shell 的 `tokengo` 系统用户；服务固定使用非 root 身份，限制写入状态和日志目录。首次启动生成主密钥，权限 0600。固定布局：

| 路径 | 内容 |
|---|---|
| `/opt/tokengo/releases/<完整commit>/` | 不可变二进制、前端、交付清单和运维脚本 |
| `/opt/tokengo/current` | 当前 release 符号链接 |
| `/etc/tokengo/tokengo.env` | root 0600 环境配置；systemd 读取 |
| `/var/lib/tokengo/tokengo.db` | SQLite，含持久账本、预算及身份 |
| `/var/lib/tokengo/master.key` | 解密上游密钥必需，不能单独重建代替旧钥匙 |
| `/var/lib/tokengo/tokengo.db.billing-outbox/` | 与该 DB 绑定的结算补偿、dead-letter/锁等文件 |
| `/var/log/tokengo/` | 诊断日志；另见 journald |
| `/var/backups/tokengo/<UTC时间-随机后缀>/` | root 0700 停机一致快照，含 state/config/unit/release 身份与校验清单 |

`TOKENGO_DB`、`TOKENGO_MASTER_KEY_FILE`、`TOKENGO_WEB_DIR`、日志路径采用随包模板。管理脚本拒绝偏离这些路径或通过 `TOKENGO_MASTER_KEY` 提供环境密钥，避免备份遗漏。当前自动化也只接受 loopback listen；需要内网地址时应显式审阅配置和对应探针后调整，不开放 `0.0.0.0`。systemd 不加载其他任意 `.env`，启动失败不会临时替换主密钥。

## 停机备份、升级与失败回滚

先暂停所有新旧入口的调用方、等待/取消活动请求，保持维护状态直到验收结束。服务收到 SIGTERM/SIGINT 后停止接受新 HTTP 请求，默认等待在途请求120秒，然后最多10秒补写队列/账务 outbox；HTTP超时或补写失败非零退出。`TOKENGO_SHUTDOWN_TIMEOUT_SECS` 可设1～3600秒，默认 systemd `TimeoutStopSec=150`，增大预算时同时延长外层停止期限。后台探针没有全局静默保证；超时或强制停止仍可能打断请求，不能把退出当作所有账务已知的证明，已有 durable intent 在启动时 replay/recover。不要在线分别复制 DB 与 outbox，也不要删除 dead-letter 来强行启动。

```bash
# 可单独制作停机快照，完成后保持停止，便于离机复制。
sudo python3 /opt/tokengo/current/deploy/manage.py backup
# 将输出的整个快照目录安全复制到离机加密备份介质，限制访问。
sudo systemctl start tokengo

# 新包先验证，再由脚本停止→一致备份→替换release→启动→检查。
sudo python3 /opt/tokengo/current/deploy/manage.py upgrade /tmp/tokengo-NEW-linux-x86_64.tar.gz
```

升级会保留旧 release 与完整停机快照；启动或本地健康检查失败时，脚本停止新版本，将 DB/master/outbox/config/unit 整组还原到同一快照，再切回旧 release 并重新检查。失败版本状态保存在 `/var/lib/tokengo.failed-*` 供核查，权限仍受限。未使用中的 release 不自动删除。

健康检查解析 `/readyz` 中真实 `status` 和数据库检查，再逐个读取 `/ui/` 下发布目录的全部文件并比较 SHA256，防止 HTML 200、SPA fallback 或缺失 WASM 被误判为前端可用。它仍不替代浏览器渲染、多用户隔离、真实模型和工具协议验收。

需要人工恢复时先核对完整快照和对应旧 release 仍在；命令会再次停止服务、保留当前停机快照，再替换整组状态：

```bash
sudo python3 /opt/tokengo/current/deploy/manage.py restore /var/backups/tokengo/SNAPSHOT
sudo python3 /opt/tokengo/current/deploy/manage.py health
```

**恢复会回退备份之后的账本、预算、身份与路由变化。** 只有维护期间没有新消费时才可直接版本回退。若已接生产流量，先对账并保留所有新状态与 outbox，不能将旧快照恢复当作无损操作。跨机器灾备也必须整组保留 master、DB/WAL/SHM、outbox 和原 release；部署到新机后显式核对文件属主为 `tokengo`、主密钥 0600，并执行同样的 startup replay、readiness 和真实调用验收。损坏/跨 DB 的 outbox 应阻止启动，不能清空补偿文件绕过。

## 切流前验收记录

在新站内逐模型录入/导入经确认的别名、原生协议、上游地址、价格、输出边界和 fallback，默认关闭不需要的推理探活。用独立测试 VKey 验证 Chat、Responses、Messages 的非流式和流式文本、完整工具多轮、取消、故障切换及账本实际/估算标记。再用真实 Codex/Claude Code 客户端验证，不以回环 mock 代替。

应同时记录新 LXC SSH/架构、Actions run 和 commit、包 SHA256、systemd 非 root 身份、重启自启、浏览器前端、旧版恢复演练，以及各 LiteLLM 模型的实际调用结果。全部通过后才为用户审阅具体入口变更和回退方案；此包不执行最终切流。
