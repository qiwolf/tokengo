# tokengo

FreeLLMAPI 的 **Rust 全量移植**。

上游：https://github.com/tashfeenahmed/freellmapi （MIT）— 服务端 `@freellmapi/server`、
面板 `@freellmapi/client`、命令行 `freellmapi`、共享类型 `@freellmapi/shared`。

本项目的目标是**一一对应地上游全部功能**用 Rust 重写，不砍功能、不做精简版。
面板同样用 Rust（编译到 WASM），不保留上游的 React。

## 现状

阶段 0：工程骨架 + 上游清点。所有端点先返回 501 占位，把**路由树**摆完整，
保证上游打得到的路径在这里都有响应，不会出现 404 假象。

| 说明 | 文件 |
|---|---|
| 上游全项目清点（规模 / 33 张表 / 31 个路由文件 / 192 个 handler / 37 个 provider 适配器 / 45 个服务 / 对外暴露面） | `docs/01-上游清点.md` |
| 上游源码（只读参照，不在本仓库内） | `/tmp/upstream-source` |
| 清点脚本（可重跑，上游更新后用来对差） | `scripts/gen_inventory.py` |

## 工作区

```
crates/
  shared/   共享类型：Platform 枚举（72 个上游平台）、请求/响应结构、错误码
  server/   网关服务端（axum）— 路由树对齐上游 server/src/routes/
  web/      面板（Leptos → WASM）— 阶段 7
  cli/      命令行（对应上游 cli/）— 阶段 8
docs/       上游清点与各阶段任务书
scripts/    工具脚本
```

## 构建

```bash
cargo build --release -p tokengo-server   # 服务端
cargo run   -p tokengo-cli -- version     # 命令行
```

面板需要额外工具链（阶段 7 接入）：

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
```

## 分阶段

| 阶段 | 内容 |
|---|---|
| 0 | 工程骨架 + 上游清点（本阶段） |
| 1 | 数据层（33 表 / 41 迁移）+ 鉴权与会话 |
| 2 | 路由骨架 + 端点全对齐（含 anthropic / ollama / gemini / responses / MCP） |
| 3 | 37 个 provider 适配器 |
| 4 | 业务机器（目录同步 / 模型发现与退役 / 评分 / 路由 / 免费池 / Fusion / 配额） |
| 5 | 11 个压缩引擎 + 响应缓存 + 幂等 |
| 6 | 媒体 / 搜索 / 审核 / 重排 / 向量 |
| 7 | 面板（Rust/WASM） |
| 8 | CLI |
| 9 | 影子并行 + 与在役网关逐字段对拍 + 切换 |

## 许可

上游为 MIT，本项目沿用 MIT。上游的许可与版权声明见 `NOTICE`。
