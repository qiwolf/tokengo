# tokengo

本版发行标识 **[v0.34.0-qiwolf.14](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.14)**，Cargo0.34.0/schema40。包含账务管理、上游模型映射移除与重新添加、共享模型候选及执行一致性修复。237 已完成独立真实推理和用量账本验收，模型按用户确认保持未定价；临时配置已清理，历史账目保留。见[版本说明](docs/releases/v0.34.0-qiwolf.14.md)。公开资产是否已发布、精确源码、校验和及镜像 digest 以该 tag 的 GitHub Release 和 manifest 为准；公开产物独立验收收据完成后补入 main 和 Release 页面，不把生产私有构建哈希当作公开产物值，也不表示下载包已包含后补收据。

历史公开版 [qiwolf.11](docs/releases/v0.34.0-qiwolf.11.md) 的既有收据保留。

历史公开版 **[v0.34.0-qiwolf.10](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.10)** 修复管理员停用Key后的删除，既有冷备升级与Key验收见[历史收据](docs/releases/v0.34.0-qiwolf.10.md)。

中文首页 | [English / 完整英文部署指南](README.en.md) | [MIT 许可证](LICENSE)

历史公开版本为 **[v0.34.0-qiwolf.9](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.9)**：Key直接交付与自动复制、撤销后列表删除、加密再次显示及普通HTTP嵌套弹窗系统剪贴板修复。Cargo0.34.0、schema40，Linux x86_64原生包与linux/amd64容器。发行源码 `8598fc070ca4a91e700d5748947d0343766ad06b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39898866368)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39898890902)均通过。9个公开资产匿名下载校验、GHCR匿名按摘要拉取、原生与容器Key运行检查及无.env固定SHA Compose启动/重启/同卷重建通过。详见[本版说明](docs/releases/v0.34.0-qiwolf.9.md)与[验收状态](docs/acceptance-status.md)。标签及公开资产内的文档是发布前源码快照；完整发布后收据保存在main分支与GitHub Release说明中，不代表先前资产已包含后补回执。

上一公开版本（历史收据）为 **[v0.34.0-qiwolf.8](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.8)**：共享向量/重排回退、逐来源自动续验、虚拟模型回收站、透传服务授权与目录展示整理。发行源码 `ca008c7acfcc53d49c5c59670dd123fc28c37a5b`；[Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/39835960738)与[打包发布](https://github.com/qiwolf/tokengo/actions/runs/39837203892)通过。Linux x86_64 原生包与 linux/amd64 容器已公开，9个资产匿名下载校验、GHCR按摘要匿名拉取、无.env的Compose启动以及原生/容器实际运行和重启/同卷重建通过。详见[本版说明](docs/releases/v0.34.0-qiwolf.8.md)与[验收状态](docs/acceptance-status.md)。Cargo仍为上游0.34.0。

上游 Key 管理使用独立弹窗，“编辑 Key”与“选择模型”分开。已有 API Key 可由管理员显式查看或复制，编辑留空保留原值。点击上游模型输入框即读取当前 Key 的目录，搜索、多选后生成上游名称与本地映射对照，也可手工输入；本地映射留空沿用上游名。既有映射保持原名，详见[操作说明](docs/provider-key-pools.md)。

本 fork 基于上游 v0.34.0，使用 Rust / Axum / SQLite 与 Leptos / WebAssembly 构建个人或小团队网关。已实现多用户、多 VKey、预算与持久账本、有序 fallback、三协议原生入口，以及账号密码登录。统一用户、自助 Key 和点选权限的第一阶段已实现，最终界面、CI和部署验收状态单列。最新分支增加在线单地址与本地多地址渠道、多上游Key池、用户私有虚拟模型与顺序/时段策略，以及非破坏模型开关和原生推理档位元数据管理。

本分支已实现不等于生产验收完成；生产验收状态见[公开验收状态与边界](docs/acceptance-status.md)。Cargo 版本仍为 **0.34.0**，以审阅提交和对应 CI 为准；fork 分发标签使用 `v0.34.0-qiwolf.N`，可下载版本以 GitHub Releases 为准。更新内容见 [CHANGELOG](CHANGELOG.md)。

自qiwolf.6提供[通用 HTTP 透传](docs/passthrough.md)：使用独立 VKey 路由授权，将人脸、CLIP 等原生 HTTP 接口接入同一网关；请求正文不解析，凭据由服务端注入，沿用预算及按次账本。部署和调用方切流按对应版本验收记录核对，不从保存配置推定业务可用。
透传服务使用独立管理入口，可新增、管理和删除；普通用户仅看到获准服务。用户与 Key 的 `allowed_services` 逐层收窄，保留旧逐 Key 白名单的兼容语义，撤权及排队后的调用重新鉴权；服务目录不会暴露上游凭据。详见[服务授权合同](docs/passthrough-user-access.md)。

Rust 编写的自托管大模型网关。将多个上游渠道统一到一个 OpenAI 兼容入口，管理模型别名、重试切换、客户端密钥及请求记录。服务端采用 Axum + SQLite，管理面板采用 Leptos / WebAssembly，不需要 Node.js 运行时或 npm 构建。

## 中文使用指南

### 版本与范围

Cargo 版本号保留上游 **v0.34.0**；本分支的实际功能以提交及文档为准。服务端与前端须从同一提交构建。

提供 Chat、Responses、Messages、向量、重排、模型清单与独立 Search/Fetch 接口。源协议、转换、图像及内联语音能力须显式配置，不按模型名猜测。流式提交前可切换允许的备用源，提交后不重放；取消、未知用量与来源错误分别记录。

管理面板包括总览、服务商渠道、模型、共享虚拟模型、透传服务、探针、用户、账务和设置；普通账号进入“我的空间”和“调用策略”。管理员在一个“用户”入口创建登录账号并配置授权与费用上限；普通用户登录后自行管理本人的 API Key。内部凭证与权益分表保留，主界面不再要求手动关联两套用户；根管理员仍是独立安全身份。总览显示真实请求指标、运行中请求及按类型/渠道/模型/客户端分组，账务显示价格、预算与逐尝试账本。当前以指标和明细列表为主，不把尚未实现的完整成本趋势图或 BI 分析称为现有能力；本地配额也不是多实例共享配额服务。

当前转换只覆盖可保真的文本与客户端函数工具。原生签名/不透明状态有归属约束，不能跨协议搬运；Responses 要求完整历史重放。未实现 Gemini 原生协议、独立音视频/文件 API 或任意订阅转换。官方 OAuth 预览也不接受部分普通 API 参数，详见对应文档。

### 服务商渠道、模型与原生参数

在线渠道对应供应方和一个统一 API URL；本地渠道允许同一供应方配置多个 API URL（包括同 IP、不同端口），各地址分别管理凭据和模型。渠道内管理多个上游Key及其模型。支持逐个切换或并发分配；同一用户的多个下游Key保持上游Key粘性，可按显式429/400规则切换。不同URL或认证适配语义的旧来源会拆成独立入口，原来源ID、授权、价格、OAuth和历史保留；没有固定“六个渠道”的目录限制。详见[渠道与Key池](docs/provider-key-pools.md)。

在服务商内启用/停用模型会原子更新明确列出的连接范围，停用保留上游模型名、视觉能力及字段配置。模型页默认汇总公开、启用且可路由的模型，停用项与兼容隐藏项可筛选恢复。服务商内模型开关、连接开关、整个服务商开关及全局逻辑模型开关各有不同作用域。

推理档位通过 `reasoning.effort`（Responses）或 `reasoning_effort`（Chat）表达，`service_tier` 是独立参数；客户端显式值优先。新增/编辑模型读取普通来源目录或绑定 OAuth 账号的官方目录，具体 levels、默认值及来源照实展示，不自动发送付费推理。“支持推理”这一 boolean 不足以推断档位；未知保持未知，允许明确人工确认并标记来源。目录声明未经推理实测，跨协议支持以接入协议为准。

本轮标准 GPT 展示使用 `gpt-6.1-sol`、`gpt-6-luna`、`gpt-5.6-luna`，不把 low/high 拼成新模型。实际可用名称和档位仍以当前授权账号目录为准。已有档位兼容别名可显式隐藏，从 `/v1/models` 与 auto 排除但保留直接请求、历史及内部引用；不按后缀自动删改。完整配置见 [服务商渠道](docs/provider-channels.md)。

例如向 `POST /v1/responses` 发送下列原生请求；使用前确认该账号目录开放该模型及 low 档位。`service_tier` 若受支持应作为独立顶层字段配置，不由推理档位推断。

```json
{"model":"gpt-6-luna","stream":true,"store":false,"reasoning":{"effort":"low"},"input":[{"role":"user","content":"Hello"}]}
```

部署者的可信智能体可使用[独立运维令牌](docs/operations-tokens.md)进行脚本化管理（自 v0.34.0-qiwolf.5 提供）；旧 root 会话用法见[兼容说明](docs/account-login.md#脚本化运维)。

### 用户私有虚拟模型

普通用户可自定义私有调用名、按顺序或每周时段选择目标。向量与重排多目标链须先通过服务端的真实来源兼容检查；通过后可对连接、超时、5xx及无效输出故障切换；已识别的真实容量拒绝可逐请求继续备用来源，普通400/429仍遵守显式开关。虚拟入口不因300×4096验收样本限制文本或候选数，全文及合法参数保留，实际窗口由来源决定。18小时提前自动续验；单个来源失败只隔离该来源，身份未变且核实健康的备用来源继续服务，全部不可用或全局证据损坏时拒绝调用并提示原因。首次初始化与手动保存仍要求完整检查，兼容范围及探测成本见[向量与重排回退](docs/vector-fallback.md)。Chat既有回退规则、原生会话固定来源与提交后的流式保护保留。

管理员可创建[共享虚拟模型](docs/shared-virtual-models.md)，只向用户授权共享入口，把底层模型设为“仅内部调用”以阻止直接调用和私有策略绕过；“兼容隐藏”只控制发现，不能代替访问限制。每次尝试仍检查实际来源权限、预算与价格。共享和私有模型支持移入回收站、恢复；恢复保持停用，历史与审计保留。

模型目录按供应商与用途筛选，展示名称与 API 调用名分别显示。展示用途可独立于实际路由类型：整理音频、工具等目录不会更改请求格式、端点能力、调用标识、授权或执行顺序。详见[目录展示合同](docs/catalog-presentation.md)。

### 账号、VKey、fallback 与账务

- 控制台使用账号与密码登录，同源 HttpOnly cookie 恢复会话；不再要求浏览器手填或保存登录 token。初始管理员名为 `admin`，本人可确认当前密码后改登录名或密码，成功撤销全部会话。管理员一次创建普通用户、登录凭证及授权；历史用户可在原用户内补开登录，不改变既有 Key 和账本归属。用户停用同时阻断其登录和 API 调用。见 [账号合同](docs/account-login.md)。
- 新建用户自动生成初始密码，显示/隐藏仅切换眼睛图标；保存成功后尝试复制账号、显示名和密码到系统剪贴板，复制失败提供明确提示与手动重试。
- 一个用户可有多个 VKey，按模型、原连接、付费权限及限额隔离；父级限制与预算不能被子密钥绕过。客户端用 VKey，不用面板密码或供应商密钥。见 [身份与计量](docs/identity-metering.md)。
- 逻辑模型的有序 fallback、重试、冷却及并发限制遵守权限和预算。流式输出提交前可切源，提交后不重放；不是对任意不同模型能力的无损替换。见 [模型路由](docs/logical-routing.md)。
- Token 与 Search/Fetch 按次账本分开，未知用量/未定价不冒充 0。价格版本、原子预占和持久结算 outbox 保留审计证据，供应商最终账单仍需对账。

### 协议与有限验收范围

Chat / Responses / Messages 有原生入口及受限文本/客户端函数工具转换；明文推理与部分原生请求字段可在可表示路径转换。签名、加密推理、不透明状态和供应商角色差异不能据此宣称无损全兼容。DeepSeek 原生 Responses 的 developer 语义、Messages 忽略参数及 OpenAI OAuth 预览字段限制须分别遵循 [协议文档](docs/protocols.md)。

独立 [DeepSeek Search/Fetch](docs/deepseek-search-fetch.md) 已有有限真实请求与按次入账验证，不会自动接管 Claude Code/Codex 的内置搜索工具，也不以本地价格代替供应商最终发票。[独立 OpenAI OAuth](docs/openai-oauth.md) 已有实际授权、目录、有限文本、受控单工具客户端循环及指定转换流验证；这不代表任意订阅、模型、参数或完整 Claude/Codex 功能集均通过。新增服务商分组与档位 UI 本轮依靠隔离浏览器及合成目录验收，未为其追加付费推理。

### 自定义探针

管理员可在“探针”页通过表单新增、编辑、启停、手动执行与删除诊断，配置目标、输入、状态码、内容/维度断言、超时及周期。支持 Chat、Responses、Messages、Embeddings、已绑定 OpenAI OAuth Responses，以及显式声明的独立 Search / Fetch；只显示目标实际类型。保存不发送请求，启用后等待完整周期再运行。

探针固定请求指定接入，不修改业务 fallback 健康状态；共用来源/Key 并发门及工具账号 QPS。诊断账本记录实际 Token 或请求次数，不扣用户额度；缺价或订阅成本显示未知。OAuth 使用正常刷新链，订阅探针以超时和响应大小约束，不假装支持普通输出 Token 上限。详见[探针配置与计量](docs/custom-probes.md)。

### 协议回退与下一阶段

Chat 与 Responses 同时可用时使用本次请求的同名原生接口；跨协议无法表示的字段明确拒绝，不放进用户消息。完整交叉矩阵、字段处理和回退逻辑图见[协议选路合同](docs/protocol-routing-matrix.md)。统一用户、自助 Key、点选权限第一阶段已有实现；它没有新增 schema、用户到期或自助开关，自助创建固定最多32把活动 Key。现有上限为金额，不是原始 Token 套餐。自定义探针已实现并单独记录系统诊断成本；月套餐与累计 Token 权益仍未实现，详见[需求覆盖与缺口](docs/requirements-status.md)及[分阶段改造方案](docs/unified-user-and-quota-plan.md)。

### 构建与启动

从 `main` 获取日常源码，部署时固定已审阅的提交 SHA；不要按保留的 Cargo 版本号猜测具体功能。生产升级与源码合并分别验收。

需要 Rust 1.98 或更新版本、C 编译器与链接器（内置 SQLite 所需），以及 [Trunk](https://trunkrs.dev/)。先按操作系统安装编译依赖。Trunk 源码安装遇到依赖解析问题时可使用官方预编译版本，并校验下载文件。

```sh
git clone --branch main https://github.com/qiwolf/tokengo.git
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

服务端产物为 `target/release/tokengo-server`，面板为 `crates/web/dist/` 的完整内容。两者使用同一次源码构建，不要混用版本。本 fork 以 `v0.34.0-qiwolf.8` 等独立标签区分分发版本，可下载资产以 [GitHub Releases](https://github.com/qiwolf/tokengo/releases) 实际附件为准；提供直接部署与容器部署两种路径。qiwolf.4 起的标签发布流程推送 GHCR，并保留离线镜像包，具体以 Release 实际产物为准。

### 容器部署与分发

已提供源码多阶段 Dockerfile、非 root Compose、持久数据卷及同提交 Linux AMD64 离线镜像。qiwolf.4 起，标签发布流程另向 `ghcr.io/qiwolf/tokengo` 推送版本、提交和 `latest` 标签；离线镜像包继续保留。首次设置与升级/备份步骤见[容器部署指南](docs/container-deployment.md)。不预装生产账号或来源凭据，其他架构须另行构建。

### 首次配置和客户端接入

1. 打开 http://127.0.0.1:3160/ui/ ，首次设置管理员密码并安全保存一次性显示的网关密钥，再开放访问。显示名称留空使用默认“管理员”。
2. 在“渠道”创建服务商，将已有未分组连接显式归属，或在“管理上游 Key”中新增凭据及其模型。连接名称是内部标识；填写真实协议对应的基础地址与供应商凭据。
3. 获取真实模型目录，在连接内启用需要的标准模型，必要时设置对外名到上游名的映射；确认父服务商、连接及模型均已启用。新增目录元数据不是推理实测。
4. 依据实际声明配置用途、图片能力与原生参数；未知能力先确认，不按模型名猜测。
5. 在“用户”填写登录名、显示名、初始密码，点选模型/连接范围、付费权限和限流，按需设置明确单位的费用上限；使用费用预算前先配置真实来源价格。历史无登录用户在原详情内启用登录。普通用户随后登录“我的用量与密钥”，创建自己的 Key；Key与用户共用总额度，不能扩大总权限或额度。
6. 客户端基础地址填写 `http://127.0.0.1:3160/v1`，使用 VKey 和当前目录中允许的对外模型名。
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
cargo check --locked -p tokengo-web --target wasm32-unknown-unknown
```

测试范围包括本地 HTTP/流式夹具、权限、计量、持久恢复、协议及前端纯函数。测试和隔离浏览器通过不代表全部真实供应商或生产环境已验收；以精确提交的 CI 结果和[公开验收状态](docs/acceptance-status.md)为准，不把旧版本测试数量当作当前结论。本地 HTTP 夹具被代理拦截时，请取消测试进程的代理环境变量。
公开仓库保留脱敏历史，不公开内部报告、机器专用脚本、数据库、日志、真实凭据及编译产物。历史 README 对应旧版本；不以历史内存数字承诺当前版本性能。

通过 GitHub Issues 提交版本、复现步骤与脱敏错误，不要上传口令、密钥、数据库或敏感请求内容。这里未公布专用的私密安全报告邮箱，不要公开含凭据或敏感利用细节的信息。修复应补回归测试，面板修改还应检查 WASM 编译。

许可证为 MIT，归属见 LICENSE / NOTICE；FreeLLMAPI 为兼容行为提供参考。仅接入有权使用的上游服务，遵守服务条款和额度。

---

完整英文说明、请求示例及通用 systemd 模板见 [README.en.md](README.en.md)。

向量及重排共享虚拟入口完整保留真实后端的输入规模与扩展参数，兼容探针样本不作为接收上限；详见[模型能力与兼容证据](docs/vector-model-capabilities.md)。
