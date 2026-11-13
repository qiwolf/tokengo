# 长输入预占与来源容量 fallback 修复

2026-11-13 发起，2026-11-14 执行验收。指定生产补丁，Cargo0.34.0/schema47保持，不创建公开发行标签。

## 行为

此前普通输入把出站JSON UTF-8字节数加256直接与模型/价格token窗口比较，新增价格后长会话可在发给上游前失败；候选局部计费边界失败又直接终止整个请求。

修正后的generation输入预占取JSON字节保守上界与既有来源输入context合同的较小者。来源合同未知时保留字节预占；价格cap不能代替来源合同。没有猜测tokenizer，没有截断消息、工具或显式输出上限，实际结算继续使用供应方usage。音频/图片/opaque历史保留既有完整context预占，向量/重排批次不按单context截取。这依赖已配置来源窗口合同，不是本轮重新证明所有供应方的实际硬限制。

本地计费/来源容量不适配只跳过对应候选，推进shared/private fallback及同供应商多Key选择。首次affinity归实际合格来源；已有affinity不因请求局部不适配永久旋转。真实上游400仍遵守原开关，原生会话pin不跨来源。全部来源不足时返回具体输入/输出预占与限制。

## build-test 证据

Mac源码同步至 `/srv/build/tokengo-billing-reservation-fallback-20261113/source`，每轮记录334个任务文件的SHA256并核对无缺失、无残留、无哈希不符；没有复制真实env、私钥或登录缓存。共享Linux依赖缓存与原项目target缓存复用，源码与证据目录独立。

- 原实现输入红测2失败/2通过；首版fallback红测5失败/1通过。其中all_limits的一处失败来自测试夹具错误响应层级，不作产品缺陷红测证据。
- 最终服务端lib633通过/4既有忽略；账务18通过；fallback9、渠道协议4、协议安全11、来源池19、按次计价7、共享路由28通过。合计729通过、4忽略，0未解决失败。
- 新测试原先2处错误响应层级断言失败已修正；仅补跑受影响的新测试及尚未执行套件，不重复已绿lib/账务。最终gateway与已绿lib使用的gateway内容一致。
- 同schema部署隔离流程4项通过，覆盖成功、启动失败、部分停服失败及入口恢复后新业务写入保留。第一轮因新证据目录前缀与夹具glob不匹配失败，已修正并保留原始日志。
- Linux release在build-test构建通过，退出码0，约71秒；二进制与日志、交付tar包已取回Mac。

完整日志、源码清单、运行命令及阶段退出码位于项目上级 `review/billing-reservation-fallback-20261113/`。原先8个已部署但未提交的运行补丁保留在构建源码；本次Git只提交本次改动，不把那些补丁混入本次提交。

## 线上对照验收

部署前，build-test经公网入口使用截图会话同一Key构造合成长输入：DeepSeek1,200,313字节、Sol350,247字节，均返回400 billing_token_limit。未重放用户会话正文/工具执行。请求SHA256与脚本身份在before-live.json。

2026-11-14 00:07:09–00:07:23（北京时间）自然排空后完成同schema47更新，维护约14秒。代码提交 `30b83f4f9c4d0cf4f4b73f672be6404012356bec`，线上server SHA256 `e147c655d7cee3290f858de78653fde58c36496629a11530b5e1cffbf26fc3ff`。生产目录 `/opt/tokengo/releases/billing-reservation-fallback-30b83f4f9c4d-e147c655`。运行二进制、源码清单、公网8项HTML/JS/WASM/CSS身份核对通过，Web沿用原包；内网/反代/公网readyz均200，快照timer已恢复。

相同脚本、Key、请求JSON SHA256的部署前后对照：

|模型|请求大小|部署前|部署后|实际上游输入/输出|实际来源|
|---|---:|---|---|---|---|
|Deepseek-v4.1-flash-fallback|1,200,313 bytes|400 billing_token_limit|200，完整流式输出DEEPSEEK_RECOVERED|192036 / 8|56 主帐号，deepseek-flash|
|gpt-6.1-sol|350,247 bytes|400 billing_token_limit|200，response.completed，输出SOL_RECOVERED|60414 / 20|64 Codex小学卡，gpt-6.1-sol|

成功请求633616/633615均记录真实attempt与upstream usage。DeepSeek配置的前置付费候选未dispatch，最后由主帐号56完成；不据此声称本轮验证了每个付费候选的真实上游调用。Sol预占输入272000/output512，按实际60414/20结算0.060518元，overrun=0。DeepSeek主帐号没有现役价格，账本如实为unpriced、实际usage已记录，金额未知；没有把未定价当作免费或伪造扣费。两次验收均无残留reserved账本或超额标记。

价格表三类哈希与部署前一致，静态来源/路由配置和runtime配置保持，没有数据库恢复、旧账重写或公开发行/GitHub推送。原会话正文与工具调用未重放；验证为使用相同Key的合成长上下文，不代替用户原会话后续操作验收。

[验收摘要与原始证据哈希](evidence/ACCEPTANCE.json)。回退点 `/opt/tokengo/releases/model-pricing-auto-0d499081d8ca-6fcb381a`；若需回退，沿用关闭入口、自然排空、仅回退程序的同schema流程，不恢复业务数据库。
