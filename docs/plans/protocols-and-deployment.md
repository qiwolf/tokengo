# 路线 C 与独立部署验收

用户已授权合并 B（main a68f83d）、三协议接入及有限转换、真实工具多轮/流式/取消回归，以及在 pve03 新建独立 LXC，迁移 LiteLLM 配置后逐模型验收。GPT OAuth 留待用户配置。现有 LiteLLM 继续运行。

## 执行契约

保持 Rust/Axum/SQLite/Leptos；所有入口共用 gateway 的实时授权、用户/key/祖先限额、逐尝试预占和持久结算。协议不是供应商：渠道显式声明原生支持协议及是否允许转换，旧渠道默认 Chat。原生保留供应商扩展、签名、工具 ID 和参数；转换只承诺已实现的文本/函数工具子集，不可表示的语义在发出前拒绝。

流先有界读取完整事件，首个有效输出或成功终止前不提交；心跳不延长总等待。提交后不重放。取消在所有阶段释放上游与容量并收尾已见用量。独立协议 observer 合并 usage、识别 error/incomplete；Chat finish_reason 后继续收后置 usage。

Responses 原生支持完整 input 重放；尚无可信服务端状态绑定时 previous_response_id/conversation 明确拒绝，不能把上游句柄送到备用源。带不透明签名/加密推理的原生历史必须绑定原来源；不具备来源证据时拒绝。工具完整历史仍可跨源，但保留 call/result 关联。配置能力不等于模型行为通过。

## 工作顺序与证据

1. 确认 upstream HEAD/fork main；复核超时修复的既有回归，补 200 SSE 错误和心跳首输出前 fallback。
2. 并行 request/response codec、SSE observer/transcoder、HTTP 路由与鉴权；root 负责 gateway/源能力/预算整合和裁决。
3. 覆盖原生三协议、全部文本/工具转换pair、分片/Unicode/多行、错误前后 fallback、取消、用量与限额；整仓 native/WASM 检查与独立复核。
4. 管理操作与迁移预览：用户/VKey、预算/价格/账、源协议与路由；LiteLLM 导入保留精确模型名、参数、fallback、密钥引用，未知项单列，幂等。
5. 核实 pve03 访问、资源、模板、网络、空闲 ID；构建同commit Linux+WASM包，校验、systemd、数据库/密钥/outbox备份恢复与回滚。新 LXC 独立端口，不切旧调用方。
6. 逐模型受控真实调用：文本、工具两轮、流式、取消，向量维度/归一化与rerank索引，协议与客户端兼容；缺失账号/OAuth逐项保持未验收。重启后重复关键链路。

任何阶段的单元测试/HTTP200/配置导入都不能代替目标 LXC 的真实模型和客户端验收；未完成项不得标记为完全可用。
