# DeepSeek Search 与 Fetch

两条独立 JSON 接口走与模型相同的 VKey 验证、用户及父级权限、RPM/并发、预算、逐尝试账本和持久结算链路。它们不生成 token，不启用流式输出，也不计入 token 配额。自动类型识别、探活和视觉测试跳过这类渠道，避免后台产生无账务调用。

`POST /v1/tools/search` 接受官方 query、count、ori_question、localization、need_content。缺省路由是 `deepseek-search`；可额外传 `model` 选择另一个明确工具路由，但该字段不发给上游。`POST /v1/tools/fetch` 接受 url，缺省路由是 `deepseek-fetch`。查询运算符与有效 URL 原样保留；未知字段按官方接口约定忽略。

```sh
curl "$TOKENGO_BASE/v1/tools/search" \
  -H "Authorization: Bearer $TOKENGO_VKEY" \
  -H 'Content-Type: application/json' \
  -d '{"query":"DeepSeek site:api-docs.deepseek.com","count":1,"need_content":false}'

curl "$TOKENGO_BASE/v1/tools/fetch" \
  -H "Authorization: Bearer $TOKENGO_VKEY" \
  -H 'Content-Type: application/json' \
  -d '{"url":"https://example.com"}'
```

## 渠道与权限

建议使用独立工具渠道，base `https://api.deepseek.com/v1`，保存渠道自己的 DeepSeek API Key。管理员通过 `PUT /api/channels/{id}/tools` 显式配置：

```json
{"search":true,"fetch":false,"account_scope":"my-deepseek-account","qps":5}
```

Fetch 可使用另一渠道并设 fetch:true。属于同一 DeepSeek 账号的渠道填写相同 account_scope，Search 与 Fetch 各有独立本地限速窗口；不能根据不同 API Key 猜测它们属于不同账号。默认 5 次/秒是网关保守策略。重启后窗口重建，该策略不协调旧站或其他网关的流量。配置变更增加来源 generation，在途排队请求须重新核对。

必须通过管理员 `POST /api/request-pricing` 配置每次价格，例如 `{ "channel_id": 17, "model": "deepseek-search", "micro_per_call": 750 }`。数值单位为百万分之一美元；示例对应英文官方 USD 0.75/千次。价格版本不可修改，后续新增价格不改变既有账本快照。中文官方人民币报价不能不经确认换算成美元。

## 计费与失败

每次上游尝试先向用户、VKey 及父级预算原子预留一次价格。完整 HTTP 200 且 Search results 非空或 Fetch content 非空，记 request_units=1；空结果和已知非 200 记 0。读取中断、取消或无法确认是否产生计费结果时保持未知并保守结算预留上界，不能冒充已知零费用。重启前后的持久 outbox 重放不能重复扣款。

账本 billing_unit=request，token 字段保持 NULL；界面把按次用量、零次、未知和 token 用量分开显示。源 429 可切换配置好的同类备用源；URL 403/404 原样返回，不冷却对话模型，也不把它误判为整个账号失效。调用是否实际收费由 DeepSeek 官方账户账单最终确认，网关账本记录的是已配置价格与请求证据。

独立 Search/Fetch 转发不等于已自动接管 Claude Code/Codex 的内置网页工具。内置服务端搜索还需要协议事件、引用和单独工具费用的能力声明，不能仅放开 hosted tools 校验。

依据：[中文官方指南](https://api-docs.deepseek.com/zh-cn/guides/search/)、[英文官方指南](https://api-docs.deepseek.com/guides/search/)。2026-10-20 直接页面的账号级限速分别为 Search 100 QPS、Fetch 25 QPS；缓存搜索结果可能仍显示较旧数值，部署时应复核当前页面。
