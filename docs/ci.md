# 持续集成

工作流为 `.github/workflows/ci.yml`。分支 push 和 pull request 均执行检查；同一事件、同一分支或 PR 的新提交会取消旧运行。push 与 PR 的运行分组独立，避免互相取消。

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

工作流只授予 `contents: read`，checkout 不保留认证凭据。官方 checkout/cache actions 固定到提交 SHA；更新时应核对官方 release 和提交。缓存仅含 Cargo 依赖，按系统、任务、工具链和 Cargo.lock 分开；不缓存运行数据库、密钥、日志或整个工作目录。工作流不读取仓库 secrets，不部署、不推送代码、不发布构建产物。

当前门禁不包括 Clippy 或 rustfmt；已有警告与格式差异需要单独治理。WASM check 不生成 Trunk 发布包，也不运行浏览器交互测试。CI 通过不代表已验收真实提供商、生产部署、备份恢复或完整浏览器行为；被忽略的测试也不会因此执行。首次 GitHub Actions 运行需在工作流提交到 GitHub 后单独确认，本地结构校验不能替代远端运行结果。

参考：[GitHub Actions 工作流语法](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)、[官方 cache action](https://github.com/actions/cache)。
