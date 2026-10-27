# 持续集成

代码门禁为 [`.github/workflows/ci.yml`](../.github/workflows/ci.yml)，Linux打包与发布为 [`.github/workflows/release-check.yml`](../.github/workflows/release-check.yml)。下述代码检查对应 `ci.yml`。分支 push 和 pull request 均执行检查；同一事件、同一分支或 PR 的新提交会取消旧运行。push 与 PR 的运行分组独立，避免互相取消。

两个独立任务在 Ubuntu 24.04、Rust **1.98.0** 上执行：

| 任务 | 命令 | 验证范围 |
| --- | --- | --- |
| Native workspace tests | `cargo +1.98.0 test --locked --workspace` | 原生工作区编译、单元测试与集成测试，使用默认测试并行度 |
| Web WASM check | `cargo +1.98.0 check --locked -p tokengo-web --target wasm32-unknown-unknown` | 管理面板在浏览器目标上的类型与编译检查 |

1.98.0 对应当前 Cargo.toml 声明的最低 Rust 版本。固定该版本可及时发现依赖或代码越过最低支持范围；升级最低版本时应同步修改工作流的工具链值与任务名称。原生任务安装 `build-essential`，供 `rusqlite` 的 bundled SQLite 编译使用，不依赖系统 SQLite 库。

`--locked` 要求保留已有 Cargo.lock；锁文件需要更新时，应在本地明确更新后随变更提交。CI 不通过自动重试或改用串行测试掩盖失败。若出现时序失败，需保留失败日志并修复竞态；本地可用以下命令辅助诊断：

```sh
rustup toolchain install 1.98.0 --profile minimal --target wasm32-unknown-unknown
cargo +1.98.0 test --locked --workspace
cargo +1.98.0 check --locked -p tokengo-web --target wasm32-unknown-unknown
# 仅用于对比诊断，不能替代 CI 中的默认并行检查：
cargo +1.98.0 test --locked --workspace -- --test-threads=1
```

`ci.yml` 只授予 `contents: read`，checkout 不保留认证凭据。官方 checkout/cache actions 固定到提交 SHA；更新时应核对官方 release 和提交。缓存仅含 Cargo 依赖，按系统、任务、工具链和 Cargo.lock 分开；不缓存运行数据库、密钥、日志或整个工作目录。`ci.yml` 不读取仓库 secrets，不部署、不推送代码、不发布构建产物。发布工作流另有下述权限和外部写入行为。

当前门禁不包括 Clippy 或 rustfmt；已有警告与格式差异需要单独治理。WASM check 不生成 Trunk 发布包，也不运行浏览器交互测试。CI 通过不代表已验收真实提供商、生产部署、备份恢复或完整浏览器行为；被忽略的测试也不会因此执行。GitHub Actions 的实际运行须按精确提交确认，本地结构校验不能替代远端运行结果。

## Linux 打包与公开发布

`release-check.yml` 在pull request、`v*`标签push和手动触发时运行，使用Ubuntu 24.04、Rust 1.98.0和Trunk 0.21.14。普通分支push仅运行代码门禁，不自动执行该打包流程。

`release-check` 任务执行部署辅助脚本测试、systemd单元静态检查，构建同一提交的Linux x86_64服务端/CLI与WASM，验证包摘要和manifest，再从已验证包构建Linux AMD64容器，运行隔离容器生命周期及持久化检查。原生包和离线镜像作为Actions附件保留14天。该任务只有 `contents: read`，不会推送镜像或部署实例。

`publish` 仅在仓库 `qiwolf/tokengo` 收到名称含 `-qiwolf.` 的 `v*`标签push、且打包任务通过后运行；PR与手动触发均不会发布。该任务授予 `contents: write`、`packages: write`、`actions: read`，使用自动提供的 `GITHUB_TOKEN`，重新核对同提交包和镜像，并要求同一提交在main的push Rust/WASM CI通过。标签对应的 `docs/releases/<tag>.md` 必须存在。

通过门禁后，流程推送版本标签、`sha-<commit>`和 `latest` 到GHCR，按摘要拉取并运行隔离持久化检查，最后创建公开Release及附件。因此推送匹配的标签会触发外部发布；该工作流没有部署生产实例。Release创建在镜像推送之后，若后续步骤失败，应逐项核对已产生的镜像与Release状态，不能把整次运行视为原子发布或自动回滚。

当前公开版本及其精确提交、代码门禁和发布运行链接见[验收状态](acceptance-status.md)；不能以本页描述代替实际运行结果。

参考：[GitHub Actions 工作流语法](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)、[官方 cache action](https://github.com/actions/cache)。

## 版本文档同步

每次已授权的版本更新/发布必须同步[模型兼容核实清单](provider-verification-checklist.md)、CHANGELOG、对应版本说明与[验收状态](acceptance-status.md)，同批提交本地Git并一并推送GitHub。逐项记录真实调用、模拟/SDK、未核实及未实施范围；复用旧证据注明版本/范围，不把未补验提升为通过。发布后追加实际收据和清单状态，保留历史失败，不移动已发布标签。仅文档维护不触发重新打包或部署，必要检查通过后不重复无关测试。
