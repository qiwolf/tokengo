# 模型兼容核实清单

## 2026-11-04 增量：v0.34.0-qiwolf.29（生产gate后正式发行，公开验收通过）

|项|已确认依据|当前边界|
|---|---|---|
|管理员渠道显示|recent/detail/attempt采用供应商·Key原备注，透传服务名保留；生产合法管理员cookie实测recent/detail新label，按渠道另核一致|不改原名/备注/模型/连接ID，当前名不是历史快照，不把按渠道旁证说成特定付费来源recent见证|
|普通用户总览|独立owner账号API及安全DTO、本人三维/分页/详情，隔离A/B/admin真实Chrome九场景和API矩阵通过；生产匿名三端点401|未冒用生产普通用户登录；不把前端隐藏或管理员截图当owner过滤证据，不返回上游/其他用户字段|
|最后布局|5d仅usage-note class与专属CSS；新Web185/WASM186.71秒及桌面1470/窄屏600的24/16px文字对齐实测通过|server字节与权限逻辑输入不变，完整角色矩阵按此边界复用；新Web字节另验，不冒称与旧Web一致|
|迁移与本地|schema44仅owner索引，生产旧数据守恒；旧q28程序读写44合成库、server lib564/0/2、routes28、owner API7与新配套包通过|维护MAX43→44旧测试期待，保投影/NULL/历史守恒；原生按已核输入/字节缓存复用，不称生产回退已演练|
|正式CI/发行|[精确CI](https://github.com/qiwolf/tokengo/actions/runs/41836872266)Rust1603/0/9、Python13/WASM，[发行流程](https://github.com/qiwolf/tokengo/actions/runs/41836871840)与9资产/GHCR/原生容器生命周期/清理通过|重叠不累加；公开fresh44/60表、owner索引及43/44 SQL/7字段另核，不把公开静态WASM当浏览器/owner行为实测|
|现有调用保护|最小本地BGE1024/CLIP512前后2+2实调HTTP200/业务/账本/响应hash与归因保持，0模型重试、0付费/额外UI推理|初始账本观察字段缺失与远端未启动错误均留证只读核查，不重发模型；无新原厂能力或历史0%成功率计数变更|

证据：[说明](releases/v0.34.0-qiwolf.29.md)、[安全收据](acceptance/2026-11-04-qiwolf29-release/README.md)、[结构化摘要](acceptance/2026-11-04-qiwolf29-release/evidence.json)。tag5d与最终后补docs分开；不公开真实Key备注、生产请求ID、内部拓扑/路径或原始截图。以下历史兼容证据继续保留原日期、版本与协议范围。

## 2026-11-04 增量：v0.34.0-qiwolf.28（生产验收后正式发行，公开产物独立验收通过）

|项|已确认依据|当前边界|
|---|---|---|
|透传详情|真实Chrome服务名及客户端/上游路径显示、历史缺值API/回归、普通LLM合同保持|服务名为当前元数据；不猜历史、不存query，内部账务分类保留|
|Key列表|owner-scoped额度批读、一次HTTP与说明对齐在真实会话核实；零/无限/冻结/停用/缺字段及跨用户隔离有回归|单次热态网络关键链约4.82s/8请求→57ms/1请求，不代表冷缓存/端到端/p95；实际Key和额度不改|
|迁移/构建|schema43仅两nullable无默认列、生产旧数据投影守恒、合成库旧q27程序读写43、build-test定向与全新配套构建通过|公开fresh43/60表和迁移包内合同另验；不称公开QA执行生产升级或生产回退演练|
|正式CI/发行|[精确CI](https://github.com/qiwolf/tokengo/actions/runs/41685780070)Rust1591/0/9、Python13/WASM，[发行流程](https://github.com/qiwolf/tokengo/actions/runs/41685779973)及9资产/tag/digest/GHCR/原生容器独立验收通过|重复覆盖不累加；公开WASM静态身份与部署本地包真实浏览器证据分别记录|
|现有调用|升级前后6+6现有调用业务与账本通过，另2次CLIP仅作UI见证，共14实调、API零重试|无原厂新测，不外推全部型号/协议/工具/strict/多轮；历史0%成功率计数口径未改|

证据：[本版说明](releases/v0.34.0-qiwolf.28.md)、[安全收据](acceptance/2026-11-04-qiwolf28-release/README.md)、[结构化摘要](acceptance/2026-11-04-qiwolf28-release/evidence.json)。不公开真实Key备注、请求ID、内部路径/拓扑或原始截图；以下历史模型证据继续保留原日期、版本和协议范围。

## 2026-11-03 增量：v0.34.0-qiwolf.27（生产验收后正式发行，公开产物独立验收通过）

|项|已确认依据|当前边界|
|---|---|---|
|按渠道展示|独立display_label、归属一致的供应商/Key备注、透传保名、连接编号与说明及按模型维度独立已在真实UI核实|仍按连接ID分组，不合并共享Key连接；备注可能含模型名，不改保存值或原始日志/模型归因|
|定向与全量|build-test28路由/178 Web、新优化WASM和原生配套包通过；[精确CI](https://github.com/qiwolf/tokengo/actions/runs/41644499984)Rust1584/0/9、MinerU Python13及WASM通过|206定向与全量重复覆盖不相加，未重复build-test全量|
|生产现有调用|升级前后6项人脸/CLIP文本和图片/OCR/BGE/原有本地聊天均业务合同与账本通过|不是新原厂/全部型号、工具、strict、多轮或全部协议验收，透传既有成功率及历史0%口径未改|
|正式发行|[Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.27)于北京时间2026-11-03 21:39:55（UTC 2026-11-03T13:39:55Z）公开9资产；[发行流程](https://github.com/qiwolf/tokengo/actions/runs/41644506211)和9资产/digest/包内身份、匿名GHCR实际拉取及原生容器生命周期独立验收通过|公开WASM仅静态检查，真实UI针对部署的build-test配套包；公开CI包与本地包分别记SHA|

证据：[实现记录](maintenance/provider-key-label-20261103/README.md)、[版本说明](releases/v0.34.0-qiwolf.27.md)、[发行与生产收据](acceptance/2026-11-03-qiwolf27-release/README.md)、[结构化摘要](acceptance/2026-11-03-qiwolf27-release/evidence.json)。不公开真实Key备注、生产账本ID、内部拓扑或原始遥测；以下历史原厂证据继续保留原日期/版本边界。

## 2026-11-03 增量：v0.34.0-qiwolf.26（已正式发行、公开产物独立验收及237配套部署通过）

|项|已确认依据|当前边界|
|---|---|---|
|透传名称与缺名|统计按全部transport读取真实名称，含停用服务；真实Chrome总览正确显示GPU03 CLIP/FACE，缺名不推断删除|当前元数据不是历史名称快照；LLM目录/权限不扩大，透传成功率口径未改|
|回归与构建|build-test26路由/177 Web及优化WASM通过；精确c939的GitHub全量Rust1581/0/9、MinerU Python13及WASM通过；本地原生打包校验及日志产物回收通过|203定向与全量重叠不累加；WASM按一致输入/28资产hash复用，不重复无关测试|
|237现有调用|升级前后6项合成调用均HTTP200/attempt200：Face1检测512维，CLIP文本/图片512维、OCR marker、BGE1024维、本地Qwen Chat非空stop|不是原厂付费模型或全部型号实测；不外推Responses、工具、strict、多轮或其他推理档位|
|公开发行|[正式Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.26)于2026-11-03T12:49:41Z公开9资产；[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/41637257354)和[发行流程](https://github.com/qiwolf/tokengo/actions/runs/41637262505)成功；9资产匿名下载/digest/包内manifest、GHCR匿名实际拉取及原生/容器/Key/Compose生命周期独立验收通过|公开WASM仅静态身份；真实浏览器验收针对237部署本地包，公开CI包与本地包分别记SHA；旧原厂证据保留原日期版本|

证据：[修复记录](maintenance/passthrough-label-20261103/README.md)、[版本说明](releases/v0.34.0-qiwolf.26.md)、[发行及237收据](acceptance/2026-11-03-qiwolf26-release/README.md)、[结构化摘要](acceptance/2026-11-03-qiwolf26-release/evidence.json)。以下q25及更早证据保持原日期、版本和适用范围。

## 2026-11-03 增量：v0.34.0-qiwolf.25（已正式发行，公开产物独立验收通过）

|项|已确认依据|当前边界|
|---|---|---|
|服务商预设与模型目录|OpenCode Go/按量、CommandGo、OpenDesign四预设及来源限定的OpenCode共享目录适配已交付；新建Go API凭据初始化独立User-Agent及稳定会话模板头，已有配置保留|预设不授予模型能力；此前只读目录和浏览器证据按原记录引用；Responses仍应传稳定显式会话ID，不扩展匿名会话规则|
|请求渠道可见性|recent/detail新增可空channel_name；虚拟入口失败切换、逻辑模型/上游别名、JSON/SSE成功与失败、删除渠道保留ID及安全日志均有合成上游回归|名称取当前渠道配置，不是历史名称快照；served_model仍指逻辑模型，上游别名见attempts及运行日志；历史文件日志不回填|
|先前定向与页面验证|build-test服务端定向751通过/2项原有忽略、Web176通过，release WASM构建成功；1440/390合成API浏览器通过，8张截图，无水平溢出及浏览器错误|该行751项为定向覆盖，不与后续完整workspace及此前预设238项重复累加；合成页面不是线上业务或原厂实测|
|精确源码与全量验证|源码`3368cd6a29d8`在build-test完整Rust1579/0/9、Python MinerU/部署/容器13/6/3通过；[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/41574671111)成功，Rust同为1579/0/9|GitHub与build-test及先前定向结果重复覆盖，不累加；本地原生构建尚在进行|
|真实来源与公开发行|[发行工作流](https://github.com/qiwolf/tokengo/actions/runs/41574711093)成功；[正式Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.25)于2026-11-03T04:03:28Z公开9资产；Cargo0.34.0/schema42不变，无新原厂模型验收|9资产匿名下载/digest/包内校验、GHCR按digest实际拉取和原生/容器/Key/Compose生命周期独立验收通过，生产未升级；公开WASM仅核对静态身份，浏览器沿用d631011一致源码证据；OpenCode此前生成仍受月额度HTTP429阻塞，不外推生成/工具/参数|

证据：[预设原交付](maintenance/provider-presets-20261102/README.md)、[OpenCode现场记录及失败边界](maintenance/opencode-go-20261102/README.md)、[请求渠道交付](maintenance/request-channel-20261103/README.md)、[本轮定向验证摘要](maintenance/request-channel-20261103/validation.json)、[版本说明](releases/v0.34.0-qiwolf.25.md)、[本轮发行收据](acceptance/2026-11-03-qiwolf25-release/README.md)。以下历史版本保持原范围，不冒充本版新测。

## 2026-11-02 增量：v0.34.0-qiwolf.24（已正式发行及公开产物独立验收；237 来源 IP 配置已上线）

|项|已确认依据|当前边界|
|---|---|---|
|生产来源 IP 配置|237 Nginx 仅信任受控 VPS 的转发地址，公网健康请求在两层日志来源一致；公网伪造头及不可信直连检查、语法检查、平滑重载和服务健康通过|是反代配置验收，未新增应用可信代理规则；未单独取得新增公网模型请求数据库记录，未发起付费模型推理；历史请求不回写|
|测试维护|main 的 `0e393f8` 将行为截止与测试外层 watchdog 分开，既有 [CI 41200992060](https://github.com/qiwolf/tokengo/actions/runs/41200992060)通过|旧提交 CI 不代替本次精确发行提交、新包或生产整包验收；生产超时/回退逻辑未改|
|模型能力与真实厂商|继续沿用下方 qiwolf.23/qiwolf.22 的来源、精确型号、协议和认证范围|本次没有新增或重新实测模型能力，不扩大 none、工具、strict、历史状态或 SDK 支持范围|
|公开发行与部署|精确提交 main CI、发行工作流、9 公开资产和匿名 GHCR/原生/容器独立验收通过；Cargo0.34.0/schema42不变|本版真实厂商与模型推理仍沿用注明版本的历史证据；237 已上线 Nginx 配置不等于新版本程序整包升级|

证据：[237 现场记录](maintenance/source-ip-20261102/README.md)、[配置验收摘要](maintenance/source-ip-20261102/acceptance.json)、[来源 IP 指南](reverse-proxy-client-ip.md)、[本版说明](releases/v0.34.0-qiwolf.24.md)、[本轮发行收据](acceptance/2026-11-02-qiwolf24-release/README.md)。以下各版保留原日期和原范围，不冒称本版新测。

## 2026-10-29 增量：v0.34.0-qiwolf.23（已正式发行及生产部署，公开资产独立验收通过）

|项|已确认依据|当前边界|
|---|---|---|
|OpenAI OAuth、精确`gpt-6.1-terra`的`none`|当前账号目录HTTP200但不含none；同账号同出口直接Responses最小流式请求200/completed，reasoning_tokens=0；生产DSH Terra Default两个对应请求成功|只对已验证连接与来源身份补充，不推广到其他账号/接入/Sol、工具或多轮|
|自定义推理档位与默认|线上追加light/default light及覆盖high/default high一次保存；无效默认显示错误且数据库不变；临时连接清理、0模型请求；隔离浏览器另证明无效默认不发保存请求|人工追加/覆盖不标为上游实测；线上未捕获网络，不声称线上零HTTP；生产回滚未实际演练|
|构建与生产|精确源码5c6efa71da32完整Rust1570/0/9、Python13/6/3、native release及WASM check通过；同源码完整native+web冷备切换、程序/Web身份及配置保全核对通过|生产Web沿用既有WASM release产物，69个前端/共享文件一致；不与202项定向结果相加|
|公开发行|产品源码5c6efa71da32803c6624f91515c01b323bc26c72；[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40884046534)及[发行流程](https://github.com/qiwolf/tokengo/actions/runs/40884478643)成功，[正式Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.23)公开9资产|公开9资产匿名下载与digest/包内校验、GHCR按digest实际拉取、原生/容器及Key独立验收通过；公开WASM另核实静态身份，不冒称浏览器行为复测|

证据：[本轮验收收据](acceptance/2026-10-29-qiwolf23-release/README.md)、[来源限定修复](maintenance/terra-none-20261029/README.md)、[直接上游脱敏收据](maintenance/terra-none-20261029/upstream-receipt.json)、[UI隔离验证](maintenance/reasoning-ui-20261029/README.md)、[版本说明](releases/v0.34.0-qiwolf.23.md)。以下既有qiwolf.22记录保持历史范围，不冒充本版新测。

以下q22基线更新日期：2026-10-28。对应历史发行版：[v0.34.0-qiwolf.22](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.22)，产品源码 `f85eca9d67801a970ba93312a553da602deacea3`。

本清单区分真实来源调用、源码/模拟上游/SDK对照、未充分验收和尚未实施。20项核心工作实现及本次交付完成，不代表所有厂商、账号、型号和参数组合均已实测。未充分验收只标注，不据此停用现有可表达功能；真实权限、归属、明确转换开关及具体无法表达请求的校验继续有效。

## 1. 已用真实模型核实

|来源与精确型号|已核实范围|不能外推的范围|
|---|---|---|
|硅基流动 `Qwen/Qwen3.8-27B`|8次网关调用：Chat/Responses客户端，普通/流式，工具调用及结果续接，前导developer处理|供应商强制strict、其他Qwen型号、百炼官方接口|
|硅基流动 `deepseek-ai/DeepSeek-V4-Flash`|2次原生Chat JSON/SSE，前导角色适配、用量和单次结算|Responses、工具多轮、全部推理控制|
|硅基流动 `zai-org/GLM-5.2`|2次原生Chat JSON/SSE，前导角色适配、用量和单次结算|GLM官方接口、Coding Plan、工具及其他协议|
|硅基流动 `moonshotai/Kimi-K2.7-Code`|2次原生Chat JSON/SSE，前导角色适配、用量和单次结算|Kimi官方Responses、summary历史、工具多轮|
|DeepSeek官方，实际上游 `deepseek-flash`|237公网Chat和Responses客户端普通问答返回预期答案，完整终态、响应usage与账本一致；各1次上游尝试/1条终态账本、0遗留预占|这里的Responses由TokenGo转换为上游Chat，不能算官方原生 `/responses` 已实测|

DeepSeek角色对照：官方直接developer为422；相同单条控制经TokenGo适配后正确返回，支持角色改写生效。任意字面标识复述样本未全部通过，官方直连也出现改写/解释标识；该项仍未充分验收。普通问答通过不覆盖此未知，没有停用模型或Responses，没有为追求测试标识而改写模型答案。

证据：[第四批验收](acceptance/2026-10-28-provider-p1-batch4.md)、[发行及237收据](acceptance/2026-10-28-qiwolf22-release/README.md)、[结构化摘要](acceptance/2026-10-28-qiwolf22-release/evidence.json)。SF共14次网关验收，不将此前直接来源诊断或官方DeepSeek对照混作这14次，也不称其为全部供应商调用数。

## 2. 已完成源码、模拟上游或SDK核实

以下均已实现并有注明范围的测试证据；“已通过”不自动提升为原厂真实账号验收。

|核心项|已核实内容|仍缺少的真实核实|
|---|---|---|
|G01 输出预算|答案/总输出区分、最终出站计量、未知原生使用注明来源的本地估算；权限、额度及超额处理保留|百炼官方实际消耗；估算不是供应方硬上限|
|G02 签名/密文归属|JSON/SSE历史、错Key/会话/来源/代次、变造及跨协议负例|Gemini/Ark官方真实签名材料|
|G03 Ark版本边界|冲突显式记录、未知版本不借旧规则授予能力、合法原生接口保持可用|文档版本冲突仍未解决；真实滚动修订和签名合同未知|
|G04 工具strict|省略/true/false三态、schema保留、具体不可表示请求拒绝|供应商是否实际强制strict|
|G05 角色适配|精确来源/型号规则、文本/文本块、顺序/字段保留、失败不改原请求；真实范围见上表|未接入型号及全部原厂角色行为|
|G06 百炼workspace|六区域识别、近似域不误认、手工配置优先|真实workspace账号调用|
|G07 DeepSeek参数|Chat与原生Responses分线、关闭思考/工具组合、Beta strict端点边界|官方原生Responses与Beta strict真实调用|
|G08 百炼工具组合|thinking、required/named/parallel等按型号与模式处理，不静默降级|百炼真实账号/版本的对应组合|
|G09 Kimi Responses历史|完整output顺序、summary/密文区分、来源绑定、变造拒绝|官方账号真实多轮及原生失败返回|
|G10 用量|cache/reasoning子集不重复加总、缺项不伪造零、重复终态不重复计费、保留有界raw usage|供应商账单逐笔对账|
|G11/G13 GLM产品线/组合|公共API与Coding Plan分开，格式/工具/采样等交叉约束|官方各产品线真实工具流|
|G12 GLM中性元数据|request_id类型/范围、Messages命名空间、早/晚SSE trace；不把未知语义静默丢弃|官方GLM账号；SDK最终对象自动合并晚到扩展没有承诺|
|G14 Anthropic|精确型号/API版本、adaptive/legacy、预算/采样/强制工具组合|官方账号、真实签名思考及全部组合|
|G15/G16 MiniMax|默认思考差异，Chat split与Responses思考/工具历史和SSE转换；strict原值保留|官方账号、strict强制效果及全部方言|
|G17 Ollama/vLLM|身份声明/变更隔离、标准API采集、保存不联网、不自动授予能力|真实部署版本、template/parser/generation配置|
|G18 兼容矩阵/SDK|空工具数组、参数分片、重复/缺失usage、异常终态、截断JSON、流内错误；分块一致；OpenAI Python3.40.0的24对照和Anthropic Python2.3.0的3对照通过|所有厂商真实异常样本和全部客户端版本；SDK对照是合成loopback|
|G19 证据/字段处置|精确身份匹配、四层证据、实际转换/默认/校验/预算原子流水线、固定路径无秘密记录|未取得的文档原文hash和真实模型revision继续unknown|
|G20 诊断/探针/界面|用途与执行种类分开；读取/保存不联网；15秒截止、取消、stale、权限/脱敏；Chat/向量/重排专用探针走通合成上游、保存不执行、账本；焦点/关闭重开/四视口|真实OAuth、全部原厂诊断、真实向量/重排来源效果及排序质量|

对应[逐项审计](acceptance/2026-10-27-provider-core-audit.md)。最终源码CI：Rust1560通过、0失败、9忽略；MinerU Python13通过，WASM通过。受影响服务端699与Web171等定向结果与全量有重叠，不相加。

### CI中的9项忽略如何解释

- 两项6万条性能基准：本轮未列为已补验。
- SIGTERM子进程helper：由父测试调用，不是独立未测试的产品功能。
- 外部真实registry获取：本轮未列为通过；公开发行资产/镜像下载的通过不能替代它。
- 浏览器harness：已另行执行真实受控浏览器验收。
- 三个SF手动测试入口：Qwen8次及所选三型号各2次已另行执行；直接角色诊断另有收据，不把CI忽略等同于已执行。
- 真实云到GPU向量/重排回退：本轮未列为通过。

## 3. 已完成发行和237核实

- 同一发行提交的服务端、CLI与Web；9公开资产的大小、GitHub digest及包内校验和。
- GHCR匿名按digest拉取；原生/容器启动、重启/持久化及隔离Key生命周期。
- 237完整冷备和整包切换、运行程序身份、24静态资源本地/公网回读。
- 主密钥、环境、原unit、既有Key、路由和用户配置保留；公网普通问答、usage与对应终态账本。
- build-test日志及产物取回Mac；临时测试目录、凭据、进程、容器和卷按范围清理。
- 本地Git与GitHub同步；未修改DSH、AG/HS或其他客户端。

生产回滚仅切回程序/页面，不用旧整库覆盖新账务或会话。首次CT下载超时及后续同SHA公开包传输、原始失败样本均保留，详见[最终收据](acceptance/2026-10-28-qiwolf22-release/README.md)。

## 4. 资料核实仍有边界

研究覆盖20类来源、73条记录、71个唯一URL。55篇官方文档中50篇读到指定章节、4篇部分读取、1篇未取得正文；没有完整镜像全部原文。16个issue/PR核实了记录时状态，没有全部在真实供应方复现；关闭/合并不证明修复已发布或TokenGo已验收。原资料及缺口台账保留日期，不当作实时全量事实。

没有覆盖所有官方账号、模型版本、参数组合和客户端版本；无证据部分保持未核实。不得把托管模型成功外推原厂，把普通问答成功外推工具/strict/状态历史，把本地估算外推供应方硬cap。

## 5. 尚未实施的独立可选接入

|选项|新增接入范围|
|---|---|
|O01/G21|xAI|
|O02/G22|Mistral文本|
|O03/G23|Mistral connector|
|O04/G24|Cohere V2|
|O05/G25|OpenRouter|
|O06/G26|Gemini原生|
|O07/G27|Azure|
|O08/G28|Vertex|
|O09/G29|Bedrock|

这些是方案中的新增协议/执行器工作，当前未实施；不表示关闭这些厂商已有的兼容接口。

## 6. 每次版本更新的维护规则

1. 在用户已授权的版本更新/发布任务中，同步维护本清单、CHANGELOG、对应版本说明及验收状态；本清单与当次版本文档同批提交本地Git并一并推送GitHub。
2. 更新版本/源码/日期、受影响项、真实来源的精确型号/协议/模式、已核实/未核实/失败/未实施状态及证据链接。新增或修复能力须有对应范围，未补验不得改成已通过。
3. 沿用旧证据时注明原版本/源码、范围与沿用原因；相关代码/环境未变且已有必要检查通过时，不为更新表格重复整套测试，不冒充本版重新实测。
4. 发布前可写待验收；发布后补真实资产/运行/生产收据，再同步清单。保留历史版本记录和失败，不移动已发布标签来补文档。
5. 未充分验收只标注，不据此停用现有可表达功能；权限和具体无法表达请求仍校验。尚未实施不得写成“已实现但未测试”。
6. 单纯文档维护不自动触发新标签、重新打包或生产部署。

|版本|日期|记录|
|---|---|---|
|qiwolf.25|2026-11-03|已正式发行9资产；精确main CI/发行流程及build-test完整Rust1579/0/9、Python13/6/3通过；公开独立验收通过，本地原生/WASM发布构建、打包及校验均通过，全部日志和产物已取回Mac；无新原厂模型验收，生产未升级。|
|qiwolf.23|2026-10-29|Terra none及自定义推理设置已生产验收，精确main CI和正式发行成功；9资产已公开，独立验收通过。|
|qiwolf.22|2026-10-28|建立此持续维护清单；核心实现、分层验收、公开发行与237升级范围按上述收据记录；原厂/字面复述等未知继续保留。|
