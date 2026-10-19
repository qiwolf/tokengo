# tokengo

中文首页 | [English / 完整英文部署指南](README.en.md) | [MIT 许可证](LICENSE)

本 fork 基于上游 v0.34.0，已加入用户与虚拟密钥（VKey）、预算及逐尝试账本、逻辑模型与有序 fallback、Chat / Responses / Messages 原生入口和文本/函数工具转换。另提供 DeepSeek Search/Fetch 按次计费、原生 Chat 语音输入与独立 OpenAI OAuth。具体配置和限制见[身份与计量](docs/identity-metering.md)、[模型路由](docs/logical-routing.md)、[协议与客户端](docs/protocols.md)、[Search/Fetch](docs/deepseek-search-fetch.md)、[OAuth](docs/openai-oauth.md)和[部署指南](docs/deployment.md)。功能存在不等于任意供应商、订阅或客户端均已验收。

Rust 编写的自托管大模型网关。将多个上游渠道统一到一个 OpenAI 兼容入口，管理模型别名、重试切换、客户端密钥及请求记录。服务端采用 Axum + SQLite，管理面板采用 Leptos / WebAssembly，不需要 Node.js 运行时或 npm 构建。

## 中文使用指南

### 版本与范围

Cargo 版本号保留上游 **v0.34.0**；本分支的实际功能以提交及文档为准。服务端与前端须从同一提交构建。

提供 Chat、Responses、Messages、向量、重排、模型清单与独立 Search/Fetch 接口。源协议、转换、图像及内联语音能力须显式配置，不按模型名猜测。流式提交前可切换允许的备用源，提交后不重放；取消、未知用量与来源错误分别记录。

管理面板包括总览、渠道、模型、用户、账务和设置；日志从排障入口访问。用户/VKey 支持权限与限额，账务支持版本价格、预算及账本；不是多实例共享配额服务。

当前转换只覆盖可保真的文本与客户端函数工具。原生签名/不透明状态有归属约束，不能跨协议搬运；Responses 要求完整历史重放。未实现 Gemini 原生协议、独立音视频/文件 API 或任意订阅转换。官方 OAuth 预览也不接受部分普通 API 参数，详见对应文档。

### 构建与启动

路线 C 目前通过开发分支和 PR 交付，尚未合并到 main；下例明确选择该分支。部署时进一步固定已审阅的提交 SHA，不按 Cargo 版本号猜功能。

需要 Rust 1.98 或更新版本、C 编译器与链接器（内置 SQLite 所需），以及 [Trunk](https://trunkrs.dev/)。先按操作系统安装编译依赖。Trunk 源码安装遇到依赖解析问题时可使用官方预编译版本，并校验下载文件。

```sh
git clone --branch codex/protocols-and-deployment https://github.com/qiwolf/tokengo.git
cd tokengo
cargo build --locked --release -p tokengo-server
rustup target add wasm32-unknown-unknown
cd crates/web
trunk build --release
cd ../..
TOKENGO_LISTEN=127.0.0.1:3160 \
TOKENGO_DB=./tokengo.db \
TOKENGO_MASTER_KEY_FILE=./master.key \
TOKENGO_WEB_DIR=./crates/web/dist \
TOKENGO_LOG_DIR=./logs \
./target/release/tokengo-server
```

服务端产物为 `target/release/tokengo-server`，面板为 `crates/web/dist/` 的完整内容。两者使用同一次源码构建，不要混用版本。当前公开 Release 仅含源码说明，没有预编译生产程序或已发布容器镜像。

### 首次配置和客户端接入

1. 打开 http://127.0.0.1:3160/ui/ ，在本机先初始化面板口令，保存网关密钥，再开放访问。
2. 在“渠道”填写上游基础地址与提供商密钥。若完整接口为 `/v1/chat/completions`，基础地址一般填写到 `/v1`，不要填写整个接口路径。
3. 在渠道内添加或勾选服务的模型名，必要时填写客户端名到上游模型名的映射；确认渠道已启用。
4. 根据实际能力设置用途和图片输入支持，不按名字猜。模型页不是另一个独立模型提供商。
5. 在“用户”创建调用用户及虚拟密钥，设置模型/渠道范围、付费权限及限额；需要计费时在“账务”配置价格和预算。客户端使用虚拟密钥，不是面板口令或上游提供商密钥。
6. 客户端基础地址填写 `http://127.0.0.1:3160/v1`，模型填写已配置的客户端模型名。

```sh
export TOKENGO_API_KEY='YOUR_GATEWAY_KEY'
curl http://127.0.0.1:3160/v1/models \
  -H "Authorization: Bearer $TOKENGO_API_KEY"
curl -N http://127.0.0.1:3160/v1/chat/completions \
  -H "Authorization: Bearer $TOKENGO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-chat-model","stream":true,"messages":[{"role":"user","content":"Hello"}]}'
```

普通对话、向量和重排的完整请求示例见 [英文指南的 Client Examples](README.en.md#client-examples)。

### 配置说明

| 环境变量 | 默认值或行为 | 用途 |
| --- | --- | --- |
| TOKENGO_LISTEN | 127.0.0.1:3160 | HTTP 监听地址 |
| TOKENGO_DB | tokengo.db | SQLite 数据文件 |
| TOKENGO_MASTER_KEY_FILE | master.key | 持久化加密主密钥 |
| TOKENGO_WEB_DIR | 自动查找 | 面板目录；生产建议显式指定 |
| TOKENGO_LOG_DIR | 服务入口默认 ./logs | 运行日志目录 |
| TOKENGO_OPENAI_PROXY | 未设置则沿用 HTTP 客户端默认出站行为 | 仅 OpenAI OAuth/订阅推理专用代理，不改普通源客户端 |

相对路径以工作目录为准；生产使用绝对路径。路由、超时和数据保留通过面板设置，不是额外的 YAML 配置。

源码默认首次请求后最多重试 3 次，尝试/流式空闲超时 30 秒，非流式完整响应期限 120 秒；已保存配置可覆盖默认值。这不是每次请求总耗时的固定承诺。

### 部署、备份与升级

使用专用低权限账号和持久化数据目录，成套部署程序与完整面板。用 systemd 等工具管理进程；可直接参考 [通用 systemd 示例](README.en.md#production-deployment)，需先准备账号、目录及文件。

公网通过 HTTPS 反向代理接入本机监听。保留 Authorization 请求头，SSE 关闭响应缓冲，设置适当的长响应读取超时。先完成管理员初始化，再开放访问；必要时额外限制面板访问。

数据库与对应 `master.key` 必须成套备份，备份也按凭据保密。主密钥丢失后无法解密存储的秘密。WAL 数据库写入期间不能只复制 `.db`：使用 SQLite backup API，或者干净停服后备份持久数据。

升级前保存数据库、密钥、程序、面板和服务配置的完整备份。启动会应用数据库迁移，之后检查 `/api/ping`、`/readyz`、面板及一次真实请求。旧程序不一定能读取新数据库；回滚可能需要匹配的整套备份，且丢失备份后的新写入。先在独立实例演练恢复。

### 行为边界与排障

流式输出开始后不换渠道重发，避免重复工具调用。客户端主动取消与上游失败分开记账。上游未报告用量不等于明确报告 0，更不代表零费用。`/readyz` 只检查数据库就绪，不证明全部上游可用。

| 现象 | 优先检查 |
| --- | --- |
| 面板空白或未构建 | Trunk 构建结果、TOKENGO_WEB_DIR、完整静态资源、浏览器报错 |
| 401 | 使用有效的网关密钥，不是面板口令或上游密钥 |
| 模型 404 | 模型名匹配、渠道已启用且勾选这个模型 |
| 图片 422 | 是否有符合条件且支持图片输入的渠道，网关不会静默删图 |
| 429、502、503 | 查看请求尝试和运行警告，区分额度、凭据、网络、并发 |
| 长回答中断 | 分别核对非流式期限、流式空闲及反向代理超时 |
| 主密钥写入失败 | 目录权限、运行账号与路径，不要随意覆盖旧密钥 |
| 用量未知 | 上游可能未报告，未知不等于零消耗 |

### 测试、反馈与安全

```sh
cargo test --locked --workspace
cargo test --locked --workspace -- --test-threads=1
cargo check -p tokengo-web --target wasm32-unknown-unknown
```

v0.34.0 公开副本串行全仓 **403 项通过、1 项忽略**。首次默认并行运行有一项进行中进度断言失败；单项复跑和串行全仓通过，原因未最终定位，不宣称默认并行测试已全绿。测试不能代表已验证所有提供商、浏览器或部署环境。本地 HTTP 夹具被代理拦截时，请取消测试进程的代理环境变量。

公开仓库保留脱敏历史，不公开内部报告、机器专用脚本、数据库、日志、真实凭据及编译产物。历史 README 对应旧版本；不以历史内存数字承诺当前版本性能。

通过 GitHub Issues 提交版本、复现步骤与脱敏错误，不要上传口令、密钥、数据库或敏感请求内容。这里未公布专用的私密安全报告邮箱，不要公开含凭据或敏感利用细节的信息。修复应补回归测试，面板修改还应检查 WASM 编译。

许可证为 MIT，归属见 LICENSE / NOTICE；FreeLLMAPI 为兼容行为提供参考。仅接入有权使用的上游服务，遵守服务条款和额度。

---

## English Quick Reference


Self-hosted OpenAI-compatible LLM gateway written in Rust, with a Rust/WASM administration panel.

## Current Version

v0.34.0 separates the non-streaming completion deadline from the streaming idle timeout. Recent versions also isolate explicit upstream account faults and permit a single recovery trial, while retaining live running-request progress and startup recovery for interrupted requests.

Recent updates also include model/channel cooldown visibility, configurable model context limits, upstream capability metadata and named gateway API keys.

## Features

- Chat completions and SSE streaming, embeddings, reranking and model discovery.
- Custom upstream channels, model aliases and templated request headers.
- Priority routing, retries, failover, recovery and per-channel concurrency limits.
- Request accounting, runtime logs, model capability metadata and image-input routing.
- SQLite storage, encrypted upstream credentials and authenticated administration.
- Leptos web panel with no npm build dependency.

## Build

Requires Rust 1.98 or newer and a native C toolchain for bundled SQLite.
Install Trunk using the official instructions at https://trunkrs.dev/ for the web panel.

```sh
cargo build --locked --release -p tokengo-server
cargo test --locked --workspace
rustup target add wasm32-unknown-unknown
cd crates/web
trunk build --release
cd ../..
```

## Run

From the repository root after building the server and panel:

```sh
TOKENGO_LISTEN=127.0.0.1:3160 \
TOKENGO_DB=./tokengo.db \
TOKENGO_MASTER_KEY_FILE=./master.key \
TOKENGO_WEB_DIR=./crates/web/dist \
TOKENGO_LOG_DIR=./logs \
./target/release/tokengo-server
```

Open http://127.0.0.1:3160/ui/ and complete first-time setup. Set your own panel password and gateway API key. Add upstream channels and credentials, then select the models each channel serves. Clients use the gateway key, not an upstream key.

```sh
curl http://127.0.0.1:3160/v1/models \
  -H 'Authorization: Bearer YOUR_GATEWAY_API_KEY'
```

| Method | Path | Purpose |
| --- | --- | --- |
| POST | /v1/chat/completions | Chat and streaming chat |
| POST | /v1/embeddings | Embeddings |
| POST | /v1/rerank | Reranking |
| GET | /v1/models | Configured model list |
| GET | /livez | Liveness |
| GET | /readyz | Database readiness |

For deployment, copy the release executable and panel files to your server. Set a persistent working directory and the environment variables above. Bind to loopback behind an HTTPS reverse proxy; initialize the administrator account before exposing the service. Use a supervisor such as systemd for automatic restarts.

Back up SQLite using a consistent SQLite backup, together with master.key. Losing the master key makes stored upstream credentials unreadable. Never commit the database, key file, logs or real credentials. Logs can contain request metadata; restrict access and choose retention settings appropriately.

## Source Layout

- crates/server: Axum gateway, storage, accounting and administration API.
- crates/shared: Shared types and routing logic.
- crates/web: Leptos panel compiled to WebAssembly.
- crates/cli: Routing command-line demonstration.

This public repository retains sanitized development history. Private deployment reports, machine-specific scripts and operational data are intentionally excluded. Historical README snapshots describe earlier versions, not current setup instructions. No production binaries or credentials are distributed here.

## License

MIT. See LICENSE and NOTICE for attribution, including FreeLLMAPI, which informed compatibility behavior.
