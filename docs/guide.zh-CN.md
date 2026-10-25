# TokenGo 中文使用指南

[项目首页](../README.md) · [English guide](guide.en.md) · [版本说明](releases/v0.34.0-qiwolf.19.md) · [发行与验收](acceptance/2026-10-26-qiwolf19-release/README.md)

本文面向 **v0.34.0-qiwolf.19**：Cargo **0.34.0**、数据库 schema **41**。本版已公开发行；固定版本下载、精确源码与验收结果以 [GitHub Releases](https://github.com/qiwolf/tokengo/releases) 和[发行收据](acceptance/2026-10-26-qiwolf19-release/README.md)为准。旧版变更保存在 [CHANGELOG](../CHANGELOG.md) 与[历史版本说明](releases/)，不必先读历史回执才能开始使用。

TokenGo 将 API 服务、可接入的订阅账号与本地模型放进同一个自托管网关。Rust/Axum 服务端使用 SQLite，Leptos/WebAssembly 控制台无需 Node.js 运行时。可使用原生包或容器部署；当前公开产物为 Linux AMD64。“轻量”指部署结构，不代表固定内存、延迟或吞吐保证。

## 开始使用

部署与备份见下文；已经启动服务时，从“供应商与接入 → 新建渠道”开始。完整流程如下：

1. **家族与认证**：从带 Logo 的家族卡片选择，默认显示两行，用“更多模型”展开。家族用于组织接入，不缩减真实模型目录，也不授予能力。选择 API 接入或订阅 OAuth；本地模型服务可使用 API Key 或明确的免密方式。
2. **保存凭据**：API 接入填写实际地址、渠道名和 Key；在线渠道使用统一 API URL，本地渠道允许多个地址。OAuth 选择实际认证来源，在官方页面完成本人账号确认，再保存绑定；不能用 API Key 冒充订阅认证。
3. **选择模型**：读取当前凭据的目录，搜索、多选并保存映射；可手工输入已核实型号。先填上游名，再填可选对外名，留空沿用上游名。已有映射保持原身份和权限。读取目录不发送付费推理，也不能证明账号可以调用每个模型。
4. **参数与转发**：核对来源输入/输出 Token 上界、供应方原生协议、客户端可用协议、推理档位、默认参数及网络出口。只有有证据的能力才启用；未知值保留未知，不按品牌或模型名猜测。
5. **用户与调用**：在“用户与 Key”创建登录账号、授予模型/来源权限，按需配置价格与预算。普通用户登录后创建自己的 Key，客户端使用网关地址、该 Key 与获准的对外模型名。

首次保存前关闭不会创建空渠道。凭据保存后退出会保留已存配置，可从“管理上游 Key”继续。保存响应不确定时先关闭并刷新确认，不重复新建；系统不会为补偿一个丢失的响应自动删除可能已保存的凭据。详见[渠道流程](provider-channels.md)和[Key 池](provider-key-pools.md)。

## API、订阅与本地服务

| 线路 | 如何接入 | 需要核对 |
| --- | --- | --- |
| API 服务 / 兼容地址 | 填实际 API URL、Key 与原生协议；代理地址可显式选择参数合同 | 实际服务商、协议和模型元数据；选择家族不等于自动兼容 |
| OpenAI / Codex 订阅 | 既有浏览器 PKCE 授权，绑定账号后使用其专属目录 | 账号权益、Responses 传输和专属字段限制，见[OpenAI OAuth](openai-oauth.md) |
| Kimi Code | 设备授权，读取绑定账号目录 | 目前只开放目录确认的原生 Chat 型号 |
| MiniMax 中国 / Global | 分地区设备授权与绑定；读取当地官方公共型号快照 | 公共目录标记 `provider_public`，不代表该账号订阅权益 |
| GitHub Copilot | 设备授权；官方确认页显示公开注册应用 OpenCode | 只开放账号目录明确支持 Chat 且未禁用的模型，不把登录成功当作推理成功 |
| 本地模型 | 多地址渠道，每地址独立配置 API Key 或免密、模型与边界 | 同一 IP 不同端口不自动共享协议、模型或能力；不承接订阅 OAuth |

Kimi、MiniMax 中国/Global、Copilot 的有出处公开客户端标识随镜像和二进制内置，无需每位部署者先申请独立 Client ID；有自己的注册时可显式覆盖。OAuth 令牌由服务端加密保存，不导入用户 Cookie 或本机会话文件。

上述四条新设备线路已实际验证发起、轮询到 pending、取消及密文清除；**真实用户登录、刷新、订阅权益、模型推理与账务仍需逐账号验收**。Qwen/iFlow 旧授权停用，Gemini CLI、Claude Code 及 Antigravity 不作为已接通订阅入口。依据、配置变量、限制和取消/撤销行为见[OAuth 来源](oauth-providers.md)。

## 协议和参数怎么选

**客户端接入协议**是应用调用 TokenGo 的格式，例如 Chat `/v1/chat/completions`、Responses `/v1/responses` 或 Messages `/v1/messages`；**供应方接口协议**是 TokenGo 调用上游的格式。原生路径优先，其他入口须在当前来源允许转换且能表达请求语义时使用。不能仅按上游家族决定下游接口，也不能让未支持的字段靠静默丢弃通过。见[协议选路矩阵](protocol-routing-matrix.md)。

| 控制 | 使用原则 |
| --- | --- |
| 推理档位 | Chat 用 `reasoning_effort`，Responses 用 `reasoning.effort`；实际 levels、thinking 开关及映射按来源/认证/协议/型号匹配。boolean“支持推理”不产生 low/high 枚举。 |
| `none` | 明确关闭推理，与省略、供应商默认、low/minimal 不同。精确匹配的官方兼容规则随源码分发；人工列表、已有默认及客户端显式值保留，未知第三方不自动套用。 |
| 默认值 | 只在客户端没有明确指定原生参数时补充；档位不是新模型，不按名称后缀推断。`service_tier` 与推理强度分别处理。 |
| JSON Schema | Chat `response_format` 与 Responses `text.format` 在可保真路径转换，保留 schema/name/description/strict；无法等价表达时明确拒绝。 |
| 输出上限 | 按协议区分总输出与仅答案 Token，显式 cap 保留或报错，不能删掉或偷偷缩小后重试。省略时可应用默认与已学习边界，并在出站及账务预留前重新计算。 |
| 预算与边界 | 推理预算、候选数、来源已核实上界共同约束预留；价格不是能力证明。OpenAI OAuth 有独立 cap 与别名规则，详见其专页。 |

兼容地址可显式配置 `parameter_profile`，但选择 profile 不授予认证、订阅或实测能力。内置识别与适配规则随源码和产物分发，不依赖私有数据库补丁；控制台保存的实例配置留在自己的数据库。新安装仍需自己的凭据和真实模型边界。详见[参数合同与供应方清单](references/provider-parameter-contracts.md)、[推理关闭](reasoning-compatibility.md)。

原生签名、加密推理和不透明历史有来源归属要求，不能跨账号或协议随意搬运。Responses 要求完整历史重放。Gemini 原生 `generateContent`、独立音视频/文件 API 和所有品牌订阅兼容不在当前范围。

## 网络与代理

在接入第一步展开“网络与代理”；保存渠道后可设渠道覆盖，在参数页可设具体模型接入覆盖。优先级为：**具体接入 → 渠道 → 家族默认 → 系统网络**。每层选择继承、明确直连或 HTTP/HTTPS 代理；明确直连不会继续继承环境代理。

代理地址与认证分开填写，URL 不带用户名/密码；密码保存后不回显，换地址时明确清除或替换认证。暂不支持 SOCKS。OAuth 的设备请求、交换、刷新、目录与撤销也经过对应出口；同一账号多个绑定来源的有效策略冲突时会报错。

首次创建未保存渠道时，OAuth 按认证来源选择网络家族：OpenAI 为 GPT、Kimi 为 Kimi、MiniMax 为 MiniMax、Copilot 为“其他 / 多家族”。Copilot 授权不因所选模型家族不同而切换首次授权出口。规则和未实测范围见[网络与代理](outbound-proxy.md)。

## 路由、Key 池与虚拟模型

在线渠道的多个上游 Key 可逐个切换或按用户粘性分配；后者不是同时发送多份推理。同一用户的多个下游 Key 共享上游粘性，原生会话归属优先。429/400 切换由明确开关控制，参数错误不自动等于坏 Key。

用户可创建私有虚拟模型名、配置目标顺序或北京时间/UTC 周时段；管理员可授予共享虚拟入口并限制底层模型直接访问。每次尝试仍检查真实来源权限、价格、预算与当前策略。已提交的流不在另一来源重放，虚拟名字不扩大授权。停用保留配置。虚拟模型移入回收站后保留配置与历史，恢复后保持停用；渠道删除保留请求、价格版本与财务历史，进行中或未结算时拒绝删除。见[Key 池与私有策略](provider-key-pools.md)、[共享虚拟模型](shared-virtual-models.md)。

向量与重排的数值/输出兼容验证是可选排错诊断，只在你主动启动时执行，不会定时探测GPU。未验证、失败或过期不影响保存、正常调用和按配置回退。本地来源可选择 `local_strategy: "round_robin"` 轮转起点，云端/OAuth 粘性不变；不承诺多实例分布式负载均衡或相同 GPU 利用率。见[向量回退](vector-fallback.md)和[本地节点合同](maintenance/local-vector-balancing.md)。

配置顺序为：先准备渠道凭据与模型，再配置 Key 分配或本地节点轮询，最后创建虚拟目标链并授权。三层策略可以组合；虚拟目标按所选顺序或时段执行，目标内部再分配 Key/节点。长请求由“系统设置 → 请求等待时间”调整全局等待与切换预算；当前模型路由编辑器/API只支持真实逻辑模型，不能按虚拟名保存独立超时。详见上述配置文档。

## 用户、用量和账务

管理员统一创建登录账号和授权，普通用户在“我的空间”管理自己的 Key 与用量。用户、Key 及祖先 Key 的模型/来源/付费权限和限额取交集；创建子 Key 不能扩大权限或绕过总预算。禁用用户同时阻止登录与 API 调用，根管理员保留独立安全身份。

“运行总览”查看请求指标、运行中进度及类型/来源/模型/客户端分组；“用量与账务”分开管理价格、预算及逐次尝试明细。每个 retry/fallback 按真实来源记录预占与结算；输入/输出 Token 和 Search/Fetch 按次账分开。未知用量不等于零，未定价不是免费，网关计算也不代替供应商最终账单。累计金额预算不是月度 Token 套餐，当前没有多实例共享配额服务。见[账号与自助 Key](account-login.md)、[身份与计量](identity-metering.md)。

## 诊断与其他服务

[自定义探针](custom-probes.md)按实际目标类型定期或手动诊断，保存不立即发请求；诊断账本独立，不扣普通用户额度。探针通过不等于所有真实业务链路已验收。

[独立 Search/Fetch](deepseek-search-fetch.md)提供搜索与网页提取接口，不自动接管 CLI 内置工具。[HTTP 透传](passthrough.md)按用户/Key 服务授权接入原生 HTTP 服务，正文按其合同透传，不把任意异步资源链都视为支持。普通用户只看到获准服务，不取得上游凭据。

## 部署与维护

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

已提供源码多阶段 Dockerfile、非 root Compose、持久数据卷及同提交 Linux AMD64 离线镜像。qiwolf.4 起，标签发布流程另向 `ghcr.io/qiwolf/tokengo` 推送版本、提交和 `latest` 标签；离线镜像包继续保留。首次设置与升级/备份步骤见[容器部署指南](container-deployment.md)。不预装生产账号或来源凭据，其他架构须另行构建。

### 首次配置和客户端接入

1. 打开 http://127.0.0.1:3160/ui/，首次设置管理员密码并安全保存一次性显示的网关密钥，再开放访问。
2. 按本文[开始使用](#开始使用)完成“新建渠道”的家族、认证、模型与参数流程；需要添加备用凭据时从现有渠道的“管理上游 Key”继续。
3. 确认渠道、接入和模型均启用；为用户配置模型/来源权限、真实价格及按需金额预算。
4. 普通用户创建自己的 Key；客户端通常填写 `http://127.0.0.1:3160/v1`，使用网关 Key 和当前目录中允许的对外模型名。Messages 客户端按其约定使用网关根地址与 `/v1/messages`，不要重复拼接 `/v1`。

```sh
export TOKENGO_API_KEY='YOUR_GATEWAY_KEY'
curl http://127.0.0.1:3160/v1/models \
  -H "Authorization: Bearer $TOKENGO_API_KEY"
curl -N http://127.0.0.1:3160/v1/chat/completions \
  -H "Authorization: Bearer $TOKENGO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"my-chat-model","stream":true,"messages":[{"role":"user","content":"Hello"}]}'
```

普通对话、向量和重排的完整请求示例见 [英文指南的 Client Examples](guide.en.md#client-examples)。

### 配置说明

| 环境变量 | 默认值或行为 | 用途 |
| --- | --- | --- |
| TOKENGO_LISTEN | 127.0.0.1:3160 | HTTP 监听地址 |
| TOKENGO_DB | tokengo.db | SQLite 数据文件 |
| TOKENGO_MASTER_KEY_FILE | master.key | 持久化加密主密钥 |
| TOKENGO_WEB_DIR | 自动查找 | 面板目录；生产建议显式指定 |
| TOKENGO_LOG_DIR | 服务入口默认 ./logs | 运行日志目录 |
| TOKENGO_OPENAI_PROXY | 未设置则沿用 HTTP 客户端默认出站行为 | 兼容的 OpenAI 专用继承基线；后台显式 direct/proxy 策略优先，其他来源按其网络策略 |

相对路径以工作目录为准；生产使用绝对路径。路由、超时和数据保留通过面板设置，不是额外的 YAML 配置。

源码默认尝试/流式空闲超时 30 秒，非流式完整响应期限 120 秒；已保存配置可覆盖默认值。实际重试次数由路由配置、冷却及目标类型共同约束，不能把配置中的重试值直接理解为额外发送次数；虚拟链每个目标单次派发，随后按其切换条件继续。这不是每次请求总耗时的固定承诺。

### 部署、备份与升级

使用专用低权限账号和持久化数据目录，成套部署程序与完整面板。用 systemd 等工具管理进程；可直接参考 [通用 systemd 示例](guide.en.md#production-deployment)，需先准备账号、目录及文件。

公网通过 HTTPS 反向代理接入本机监听。保留 Authorization 请求头，SSE 关闭响应缓冲，设置适当的长响应读取超时。先完成管理员初始化，再开放访问；必要时额外限制面板访问。

数据库、对应 `master.key` 与 `.billing-outbox` / dead-letter 必须成套备份，备份也按凭据保密。主密钥丢失后无法解密存储的秘密。WAL 数据库写入期间不能只复制 `.db`：使用 SQLite backup API，或者干净停服后备份持久数据。

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

测试范围包括本地 HTTP/流式夹具、权限、计量、持久恢复、协议及前端纯函数。测试和隔离浏览器通过不代表全部真实供应商或生产环境已验收；以精确提交的 CI 结果和[公开验收状态](acceptance-status.md)为准，不把旧版本测试数量当作当前结论。本地 HTTP 夹具被代理拦截时，请取消测试进程的代理环境变量。
公开仓库保留脱敏历史，不公开内部报告、机器专用脚本、数据库、日志、真实凭据及编译产物。历史 README 对应旧版本；不以历史内存数字承诺当前版本性能。

通过 GitHub Issues 提交版本、复现步骤与脱敏错误，不要上传口令、密钥、数据库或敏感请求内容。这里未公布专用的私密安全报告邮箱，不要公开含凭据或敏感利用细节的信息。修复应补回归测试，面板修改还应检查 WASM 编译。

许可证为 MIT，归属见 LICENSE / NOTICE；FreeLLMAPI 为兼容行为提供参考。仅接入有权使用的上游服务，遵守服务条款和额度。

---

英文指南、请求示例及通用 systemd 模板见 [English guide](guide.en.md)。

向量及重排共享虚拟入口完整保留真实后端的输入规模与扩展参数，兼容探针样本不作为接收上限；详见[模型能力与兼容证据](vector-model-capabilities.md)。
