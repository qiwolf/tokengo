# tokengo

自用的 LLM 中转网关。**不是 FreeLLMAPI 的全量移植**——功能按本机实际用量取舍，
只做真在跑的两件事：对话和向量。

上游 FreeLLMAPI（MIT）只借了两个东西：**端点的响应形状**（好让现有客户端不改代码就能切过来）
和 **OpenAI 兼容的线协议**。其余的免费模型目录、37 个 provider 适配器、
41 个用不上的端点，一行都没有。

## 为什么不全量移植

现网 FreeLLMAPI（LXC109）生产库 3 天统计：

    总请求 100,003 次
    向量 86,146 次   86.1%
    对话 13,857 次   13.9%
    其余 41 个端点   0 次

全量重写要跟 89,437 行、412 个文件、381 个测试；按需做只要约 6,000–9,000 行。
取舍的逐项依据见 [`docs/02-按需功能清单.md`](docs/02-按需功能清单.md)。

## 对外端点（只有四个）

    POST /v1/chat/completions    13,857 次
    POST /v1/embeddings          86,146 次
    GET  /v1/models              只列真实可用的
    GET  /livez  /readyz         探测

不做：`/v1/rerank`、`/v1/moderations`、`/v1/search`、`/v1/responses`、`/v1/messages`、
`/v1/images`、`/v1/videos`、`/v1/audio/*`、`/v1/decisions`、Gemini `/v1beta`、
Ollama `/api/*`、MCP `/mcp`、URL token——**全部真 404**，有回归测试盯着。

## 路由策略

优先级 + 重试 + 自动回切，**没有冷却**：

    请求 auto → 按模型组的优先级顺序挑腿
    首选腿失败 → 重试 3 次（共 4 次尝试）→ 全败则切到下一优先腿
    切换后 10 分钟 → 回头试首选；成功就切回来

判失败的只有腿自己的毛病（网络错 / 5xx / 429 / 401 / 403）。
客户端自己的错（400 / 404 / 422）原样返回，**不重试、不切腿**——
否则一次写错模型名的请求会把整条链路打断。
完整规格与验收步骤见 [`docs/03-路由策略.md`](docs/03-路由策略.md)。

## 上游全部自定义添加

不做免费模型目录，也不做 provider 适配器。一条腿 = 名称 + base_url + key +
**自定义请求头** + 该腿认的上游模型名 + 优先级。

自定义请求头是**上游没有、本项目主动加的**能力：FreeLLMAPI 的 `api_keys` 表
18 个字段里没有 header 列，面板也没入口，所以 OpenCode Go 这类要求特定请求头的上游
只能靠外挂脚本中转。这里加了，且支持逐请求变化的模板（`{{uuid}}`、`{{timestamp_ms}}`）。

## 工作区

    crates/
      shared/   腿与模型组的数据结构 + 路由状态机（纯逻辑，时间由外部传入，可完整测）
      server/   网关服务端（axum）— 对外四个端点 + 面板管理面
      web/      面板（Leptos → WASM）— 5 个页面，阶段 7
      cli/      命令行
    docs/       按需功能清单、路由策略
    scripts/    工具脚本

## 构建与自检

```bash
cargo build --release -p tokengo-server
cargo test  --workspace            # 24 条测试
cargo run   -p tokengo-cli -- routing   # 把默认路由参数走一遍状态机
```

面板需要额外工具链（阶段 7）：

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
```

## 阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| 0 | 工程骨架 + 按需取舍 + 路由状态机 | 已完成 |
| 1 | 数据层（腿 / 模型组 / 记账）+ 面板会话鉴权 | |
| 2 | 三个端点实装（含重试与切换）+ 与现网逐字段对拍 | |
| 3 | 面板 5 页 | |
| 4 | 部署到 LXC + 影子并行 + 切换 | |

## 与现网 FreeLLMAPI 的差异

| 项 | 现网 | tokengo |
|---|---|---|
| 路由 | `balanced` 打分 + 随机探索，同请求可能换腿 | 优先级固定，可预测 |
| 腿失败 | 打进冷却，到期前不选 | 不冷却；失败即重试，全败才切 |
| 什么时候回来 | 冷却到期，可能回到更差的腿 | 10 分钟后主动试首选，成功才回切 |
| 客户端错（400） | 会计入冷却，能把好腿打坏 | 不计入 |
| 内存 | node 进程 229.6 MB（其中 201 MB 是 V8 堆） | 目标 40–80 MB |

## 许可

上游 FreeLLMAPI 为 MIT，本项目沿用 MIT，声明见 `NOTICE`。
