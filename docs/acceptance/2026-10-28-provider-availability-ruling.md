# 兼容可用性裁决：未充分验收不等于关闭功能

用户2026-10-28明确要求：“你可以拒绝，也可以写明未充分验收，但不能关闭相关功能，要能够使用，使用中如果有问题再修正”。本裁决优先于原方案/验收中“仅因缺少文档或实测而关闭”的保守默认。具体不支持参数、协议无法表达、身份/授权/opaque归属、真实账务授权和已确认矛盾请求的拒绝仍独立判断，不删除这些边界冒充兼容。

## 本批明确修订

MiniMax split Chat↔公共Responses的函数strict不再仅因未取得enforcement实测而拒绝。generic converter的omitted/true/false语义继续执行；closed schema可证明的省略值按既有规则物化true，显式true不降为false，schema不重写，不确定严格化仍是具体不可表达请求错误。真实上游是否强制schema明确标“未充分验收”；参数意图原样送出，实际拒绝或错误另作来源归因。原生Chat同协议工具与未知字段继续保留。

已可表示的raw `reasoning_content`转换续轮不再因为 `chat_reasoning_replay` 省略或false而整项被挡或被迫跳源。原字段注释及序列化是能力声明：true表示已声明接受，false/省略表示未确认，不是独立关闭开关。字段名、类型、既有配置值均不改，也不把false偷偷改true。真实转换控制仍是 `allow_conversion`：管理员明确关闭转换继续生效；原生Chat不受它影响。

这是用户新要求下对旧默认语义的明确变更，不能声称过去false从未阻止过请求。历史bool使用serde(default)+skipfalse，显式false与缺省保存后常同形，不能从现有记录可靠恢复更细的历史意图。本批不新增伪迁移或宣称已辨明用户旧选择。管理员如需关闭转换，继续使用已有明确 `allow_conversion:false`；不会通过能力证据状态自动关闭可表示的raw续轮。

raw思考、摘要、签名和密文仍区分：不造summary、不生成空signature、不删除opaque字段，不跨来源/账号/会话借用证明。无法表达的signed Messages/no-split具体请求、unknown schema及真实参数冲突仍可返回错误。native本地估算的后续修订见下文；供应方硬上界证据与本地预占、实际结算分别记录。

成功和失败仍按实际来源记录。本批为未确认raw续轮及MiniMax strict增加固定脱敏attempt说明，不回显消息/Key，不将unknown提升为verified。没有客户端修改，也没有修改DSH或其他客户端能力开关。

## 验证入口与证据边界

新增/修订的行为回归包括：MiniMax strict三态+schema原值；省略strict的不可判定schema仍拒绝；JSON/SSE工具首轮与完整raw历史续轮三态组合；原生MiniMax strict=true/未知字段保持；raw声明缺省/false皆实际发到HTTP上游且tool ID原形；不为缺声明强制fallback；明确allow_conversion=false仍挡转换、原生同body仍可用。原有opaque归属、跨协议不可表达及计量守恒回归继续执行。

所有构建和测试由root在build-test同步最终源码后运行。本裁决落盘时尚未取得这轮新行为的最终结果，不能拿旧保守gate测试的通过当新行为验收成功。旧第三批收据保留当时真实状态，并注明本裁决替代其strict门禁和缺声明阻止raw转换的当前政策。

## 对SF验收口径的影响

没有跑满8call、目录名漂移或缺少参数文档只表示未充分验收，不表示相应family被关闭。root继续用当前账号的精确来源做真实小probe；系统正常可调用与是否启用某个Qwen专属角色/思考适配分别记录。已取得的Kimi精确SF来源developer拒绝/system接受可支持窄适配候选，DeepSeek/GLM必须按各自实际对照决定，不扩品牌匹配。

## 精确SF角色实现补充

两轮direct对照后仅窄扩SF的 `deepseek-ai/DeepSeek-V4-Flash`、`zai-org/GLM-5.2`、`moonshotai/Kimi-K2.7-Code`：前导纯文本控制按顺序双换行连接到单system，明确独立优先级已折叠。中途控制消息、不可保留的name/元数据/多模态仍是具体不可表达请求错误；未知型号及Pro变体不匹配，不从品牌名称加新参数能力。direct观察及hash见[SF补充](../references/provider-compatibility-20261027/w11-siliconflow-supplement.md)。

新增 `live_siliconflow_selected_native_role_json_sse` 为独立2call入口，仅选上述三个已实证精确ID，client保留developer唯一指令、user只发Proceed，不发送none，验证nativeChat JSON/SSE与实际usage/唯一结算。fixture不新增source context额度，复用原Price输出1024作为本地准入估计，明确不当供应方cap保证；原Qwen8call未改变。真实调用由root受控执行，此处不预先宣布通过。

## 中性追踪元数据不能关闭Messages路线

普通响应仅因request_id就拒绝转Messages会让常带trace的来源整条失败，已改为[明确的TokenGo扩展元数据](../references/provider-compatibility-20261027/w11-messages-trace.md)：JSON及message_start.message携带tokengo_metadata.request_id，晚到trace在message_delta的同名扩展保留，反向可读取。追踪ID不替换任何消息/工具/会话ID；未知语义及cache细分的具体不可表示情况仍独立拒绝，不笼统丢弃字段，也不声称已入持久化日志。

## Native缺少硬上界证据仍可使用

后续[本地预占估算修订](../references/provider-compatibility-20261027/w11-native-reservation-estimates.md)取消“仅因未知total metadata而拒绝普通native”的绑定。source配置、Price配置及原4096默认政策以带origin/confidence的ReservationEstimate复用，预准入与最终计量一致，source_snapshot/attempt明确不是supplier hardcap。具体参数/显式跨协议总cap冲突、真实Key权限、余额/限额/TPM准入与唯一结算照常执行；不修改billing扣款封顶或overrun冻结算法。TPM仍是现有本地预占口径，不承诺估算为真实token硬上界。
