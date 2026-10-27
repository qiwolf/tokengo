# 供应方兼容第四批：W10–W12 验收记录

日期：2026-10-28。本文记录已取得的源码、测试、SDK和真实来源证据；浏览器焦点及实际探针流程已取得最终收据，发行、部署按各自收据记录，不能据此宣称全部交付完成。

未充分验收是状态与范围说明，不是关闭整个模型、合法原生接口或已有可表达转换的条件。原生输出语义未知、缺少独立来源总上界时，可使用明确标注的本地预算估算；具体参数冲突、鉴权、归属、执行器及账务保护继续生效。

## 源码与执行环境

源码在 Mac 编辑，编译和测试在 `build-test` 的隔离副本执行，日志与产物已取回仓库同级 `review/provider-p1-batch4-20261027/`。本轮 phase13 的文件清单为 `root/phase13-manifest.json`，其 SHA-256：

```text
2cee5211fa646d4c8f8d25c2921cc4184e3467091cfbb9003e812600fe16f0bb
```

每条执行收据记录主机、工作目录、实际命令、退出码及源码清单摘要。phase13 的后端受影响范围为 **699 通过、0 失败、2 忽略**，共20组；不是最终全工作区全量测试。phase6 曾取得 **1535 通过、0 失败、8 忽略**，但它属于较早源码基线，不能转写为 phase13 全量通过，也不能与重叠用例累加。

## 已取得的检查结果

|范围|结果|本地收据（均相对于上述 review 目录）|
|---|---|---|
|phase13 服务端单元及受影响HTTP测试|699/0/2；退出0|`root/logs/server-affected13.log`、同名 `.json`|
|其中服务端库|552/0/2|同上|
|其中W10诊断HTTP|13/0/0|同上 `provider_diagnostics`|
|其中自定义探针HTTP|10/0/0|同上 `custom_probes`|
|其中原生预算估算|4/0/0|同上 `native_reservation_estimates`|
|phase13 Web单元|171/0/0；退出0|`root/logs/web-unit13.log`、`.json`|
|phase13 WASM检查|退出0|`root/logs/web-wasm13.log`、`.json`|
|phase13 Trunk发行构建|退出0|`root/logs/web-bundle13.log`、`.json`|
|phase13真实来源/浏览器fixture编译|退出0，仅说明编译成功|`root/logs/live-build13.log`、`.json`|
|phase11协议单元补验|67/0/0|`root/logs/trace-unit11.log`、`.json`|
|phase11协议/trace等HTTP补验|52/0/0，8组|`root/logs/trace-http11.log`、`.json`|
|OpenAI Python SDK 3.40.0 与raw HTTP对照|24/24通过，退出0|`root/openai-result11.json`、`root/logs/openai-compare11.json`|
|Anthropic Python SDK 2.3.0 对照|3/3通过，退出0|`root/anthropic-result11.json`、`root/logs/anthropic-compare11.json`|
|浏览器目标视口、焦点及实际探针|最终审计退出0；原始失败轮保留归因|`w10-ui/focus-fix14/acceptance.md`、`browser-qa14e-audit.json`、`cleanup.json`|

phase11协议/SDK导出的相关 `src/protocol/`、fixture和矩阵测试文件在 phase11→phase13 两份清单间未变化；该补验仍按自身源码范围记载。SDK均消费生产转换器导出的合成loopback数据，不能称官方供应方真实调用。Anthropic覆盖JSON、早到trace SSE、晚到trace SSE；晚到扩展在 `message_delta` 中可见，不宣称 `get_final_message` 自动合并全部晚到扩展。

## W10：可使用的诊断与验证入口

静态诊断、保存和外部验证分离。HTTP回归覆盖读取不联网、不改代次、管理权限/CSRF、脱敏、重复任务、15秒总时限、取消、来源变更失效、200错误体/无效结构，以及标准运行时采集不写回配置。Job仅保存有界内存结果，重启不保留成功证明；取消不能撤销已发生用量。

目录“配置用途”与执行 `kind/source` 独立展示；合法目录值 `chat` 显示“对话与推理”，不会因此新增推理能力、改变来源身份或调用入口。字段规则只引用当前精确合同实际适用的编号，无匹配明确为 `none`；后台执行门引用 `stateful_executor`，不冒用strict文档规则。

普通对话来源可直接发送合法原生最小请求，未知证据不禁用功能。OAuth、独立工具、向量和重排按真实来源与用途进入现有探针，预填来源、模型和请求类型；不会把OAuth当API Key、把Embedding/Rerank当Chat ping，也不会自动保存或执行。新路径沿root/session权限边界，未自动扩大运维令牌动作清单。探针通过只能说明本次协议/结构检查；重排不证明排序质量。

Ollama/vLLM的独立标准API采集与生成用途分开，Embedding/Rerank同样可采集。结果为未验证软件身份候选，不自动保存Caps、默认档位或推理级别。无真实Ollama/vLLM部署和真实OAuth来源的本轮端到端验收，不把合成账号元数据当真实授权调用。

原生未知或答案上限不含未知思考量时，W10复用共享 `ReservationEstimate`；来源配置、价格配置或既有4096默认值是本地预占估算依据，不是供应方hard cap。输出含origin/confidence及 `supplier_hard_cap_verified:false`。百炼Responses最小合法请求cap为16，这属于API请求形态，不代表未知型号已验。鉴权、schema冲突、历史归属、TPM/账务和超额处理不因估算而取消。

## W11：矩阵与真实来源

版本化合成形态仍覆盖空工具数组、同index多片参数、usage尾帧/重复/partial、length incomplete、finish后EOF、无终态EOF、截断工具JSON和流内错误；三种分块必须给相同结果。文档、供应方形态、模拟上游、真实来源分层记载。G19固定路径处置来自实际转换、默认、最终验证与预算阶段，不记录消息值、端点、头或凭据。metadata hash不冒充缺失的文档原文hash。

本轮使用合成材料对硅基流动做 **14次实际调用**，四组 `execution.json` 均退出0，`live-result.json` 均 `passed:true`、无失败；绑定同一 phase13 清单和测试二进制摘要 `e2f82be6576c3ce9209e4b6eea00b05016fc9a3f5e195ca6135deadfac349572`。

|精确托管型号|调用数|已取得的真实来源范围|
|---|---:|---|
|`Qwen/Qwen3.8-27B`|8|原生Chat后端；Chat/Responses客户端JSON、SSE及工具往返，前导纯文本developer处理|
|`deepseek-ai/DeepSeek-V4-Flash`|2|仅原生Chat JSON/SSE、前导角色折叠|
|`zai-org/GLM-5.2`|2|仅原生Chat JSON/SSE、前导角色折叠|
|`moonshotai/Kimi-K2.7-Code`|2|仅原生Chat JSON/SSE、前导角色折叠|

收据分别为 `live/{qwen,deepseek,glm,kimi}/evidence/{execution,live-result}.json`。后三组各记录2次尝试/2条请求账本、无usage不一致、无重复结算、无未结算或遗留预占；其1024输出量来自本地Price估算，供应方总cap/思考预算和价格均未验证。Qwen的成功工具参数也不能证明供应商强制strict。不得由SF结果外推原厂API、其他型号、全部推理档位或全部协议已通过。

## 尚待收口与回滚

焦点抢占与卸载后异步访问已修复。新Release包在1440/320/390/768视口、250ms延迟快速关闭重开、Tab/ShiftTab/外部焦点回收/Escape恢复、时钟偏移、重复/取消/15秒截止、metadata与保存不联网、动态stale及Chat/Embedding/Rerank探针实际执行均取得分轮真实证据。最终fresh浏览器审计退出0，三探针HTTP200且账本存在/debit0，零console/pageerror/请求失败及零新增调用。原始失败轮涉及已修复产品焦点、select标签含选项、幂等预置和续验401分类，全部原样保留；不伪改失败轮passed。临时进程、私密目录及监听已清理。62个受验Web输入逐文件匹配发行提交。W12已完成qiwolf.22发行和237整包升级，公开资产及实际公网问答/账务结果见[最终收据](2026-10-28-qiwolf22-release/README.md)。官方DeepSeek任意字面标识复述未充分验收，原失败和直连对照均保留，普通问答通过不覆盖该未知，相关功能保持可用。

无新数据库迁移。回退到不认识 `rerank` 探针类型的旧程序前，停用或移除该类新增探针；其输入继续是原 `Config.input` 文本，没有新增持久化字段。已有非空 `runtime_identity` 的旧版读取边界见[兼容维护说明](../provider-compatibility.md)。回滚不能用旧整库覆盖新会话或账务。

## 精确提交最终CI

发行源码 `f85eca9d67801a970ba93312a553da602deacea3` 的[main CI](https://github.com/qiwolf/tokengo/actions/runs/40559710708)成功：87个唯一Rust harness合计1560通过、0失败、9忽略；MinerU Python13项通过，WASM检查成功。该结果替代较早phase6基线作为最终提交全量依据；受控SDK、SF和浏览器仍按各自源码范围分层，不与CI用例数相加。build-test另完成同提交原生服务端/CLI、Release Web、部署工具测试、打包和整包校验，产物及日志已取回Mac。
