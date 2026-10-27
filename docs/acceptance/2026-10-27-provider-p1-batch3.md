# 多厂商兼容第三批：W07–W09 验收记录

起始日期：2026-10-27。基线 `5e11698f2563c702fd54f7326d2a336b47e9454f`。仅修改 TokenGo，未修改 DSH、AG/HS 或其他客户端，也未改生产配置、推送、发布或部署。本记录区分实现、厂商形态重放和真实来源调用，不把保护性拒绝说成完整兼容。

## W07：元数据与用量

GLM `request_id` 按有界字符串保留，不替换响应 id、工具 call id 或会话身份。JSON、SSE 同等处理；流内冲突、非法类型及过长值拒绝。搜索、媒体、过滤等语义扩展继续明确拒绝，不能以“兼容元数据”为由放开未知字段。[官方响应结构](https://docs.bigmodel.cn/api-reference/模型-api/对话补全)

Kimi 的缓存读、缓存写与 reasoning 数量按输入/输出总量的子集处理，不再次相加。缺少某桶保持未知，不伪造零或 TTL；冲突、负数、溢出及超出总量拒绝。JSON/SSE 统一归一，重复终态不重复计量，同一网络块含内容、多份 usage 和终止帧也必须得到相同结果。[Kimi Chat 文档](https://platform.kimi.com/docs/api/chat)

Chat↔Responses 转换在 `usage.tokengo_raw_usage={protocol,usage}` 保留有界原始观测；往返复用已有容器，不递归套娃。SSE 保留最后一份实际 usage 对象，已知计数单独累积，不把多个 partial 拼成不存在的供应商原文。原始 usage 深度最多12，容器60KiB，整体16层/64KiB；首次输入为包装预留空间，边界附近仍可往返。原生同协议保持原始字段。

这里是**响应内的原始用量留存**，不是新增持久化原报文日志库。TokenGo 请求表仍记录总量、known 标记和来源；本批未新增数据库列，也不声称已有完整供应商账单追溯。扩展是观测数据，不是可信来源证明、价格或执行能力。审计现有 gateway Usage 与 billing finish 后，没有证据要求修改结算算法；保留原预占、幂等和未知 usage 处理。Messages 无法等价表达的缓存/追踪形态继续拒绝，原生计量不改。

## W08：来源和运行时身份

新增百炼工作区域名窄匹配：单个 workspace label，加 `cn-beijing/ap-southeast-1/us-east-1/eu-central-1/ap-northeast-1/cn-hongkong.maas.aliyuncs.com`，只认官方路径。近似域、附加 label、未知区域、错误路径等不自动推断；显式 profile 优先，识别域名不授予协议或账号能力，旧域名继续有效。[百炼 Chat](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)、[Responses](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses)

`Capabilities.runtime_identity` 是可选声明，仅匹配显式 ollama/vllm profile；包含安全版本标识及最多128个精确上游模型的 revision/template/parser/generation_config SHA256。缺少组件保持未知，不保存原模板、路径或配置正文。声明只可为 `operator_declared` 或 `standard_api_claim`，二者都不是已验证；不能提交 `verified` 来提升信任。按目标模型返回有界、脱敏状态，不在每个目录项重复完整清单。

纯解析器只提取标准 API 文档能证明的字段：Ollama version/template 摘要，vLLM version/model IDs；`owned_by`、模型名和任意能力字段不证明模板/解析器。无后台扫描或保存时探测，不要求用户管理第三方后端、提供 SSH 或开放开发端点。本批未新增网络采集器，独立采集及完整诊断界面按 W10 后续实施。[Ollama API](https://docs.ollama.com/api/openai-compatibility)、[vLLM API](https://docs.vllm.ai/en/stable/serving/online_serving/openai_compatible_server/)

Ollama/vLLM 目录自称的推理档位在执行缓存和返回 DTO 中保持 unknown；管理员已有手工确认仍是独立授权。新增身份进入现有 fingerprint/binding_generation：已声明模板等变化后，旧缓存和旧手工默认失效，原配置记录不删除，不沿用至新身份。不能检测后端未上报的升级；未知身份本身不构成已验证运行环境。

现有“接入设置／高级协议 JSON”可维护该字段，切换主协议或转换开关保持声明原值；服务端执行完整字段/摘要校验，保存不联网。未新建界面或修改布局。例如可声明 `runtime_identity.runtime="ollama"`、`evidence="operator_declared"` 和实际 `models[].model`；未知 version/摘要省略，不能填示例摘要冒充核验。OAuth 绑定不接受该运行时声明。无该字段的旧配置序列化及身份保持兼容。

## W09：MiniMax split 子集与保护边界

仅在明确 MiniMax API Key 来源、精确 `MiniMax-M3` / `MiniMax-M3.1-Flash`、上游原生 Chat、下游公共 Responses 时启用：Responses 请求与 raw reasoning/tool 历史转换到 Chat，明确选择 `reasoning_split=true`；JSON/SSE 的 `reasoning_content` 转为 raw reasoning_text，不当摘要或签名。空缺思考保持空缺；普通答案里的 `<think>` 字面文本不解析、不删除。原生 Chat/同协议流量不进新 codec，no-split 和未知字段原样保留。[MiniMax Chat](https://platform.minimax.io/docs/api-reference/text-openai-api)

前导纯文本 instructions/system/developer 按序折叠为一个 system。这是语义折叠，不能保留 OpenAI 两层独立指令优先级；只合并无额外元数据的纯文本，name/未知字段/多模态/中途控制消息拒绝，不重排历史，不要求客户端删字段。工具调用 ID、结果和完整助手 raw 思考在已支持形态内保持关联。最终出站 split 不允许被来源 drop_fields 移除；显式 none、来源默认及映射默认仍按既有优先级校验，M3.1 不允许关闭。官方 max_tokens 包含思考，codec 不另造预算或把 reasoning 子集加到输出。

中性 `input/output_sensitive=false`、零分类及成功 `base_resp` 先类型/容量校验，再保留到 `tokengo_provider_metadata`；未知语义不按诊断装饰删除。敏感或非中性策略标记在 JSON/流式语义验证前识别，即使 choices 为空或正文为错误形态，也不允许自动重试或换供应商。晚到流式错误关闭输出，不追加成功终态；已有使用量独立观察并只结算一次。真实非零 base_resp 按上游错误处理，不伪造成功。

**保留门禁**：active Chat↔Messages 不能无损承载 Messages 已发出的 thinking.signature，故继续拒绝；不生成空签名，不把摘要当 raw。公开 Responses 转 Chat 的显式 no-split 拒绝；没有足够文法证据安全拆分标签。当前 MiniMax 材料也没有提供 function strict 强制执行保证，因此新转换拒绝显式或推导出的 strict=true，不静默改成 false；显式 false 工具路径可用。这不表示已证明供应商实际忽略或拒绝 strict。原生 Chat 仍按原样传递，不等于本批已验证其中所有扩展。[MiniMax Messages](https://platform.minimax.io/docs/api-reference/text-anthropic-api)

这些边界是本批明确支持的子集，不能宣称 G16 所有方言或所有 MiniMax 线路已完成。M2.x、托管同品牌、OAuth、MiniMax 原生 Responses、签名跨协议均未借型号名称自动开放。

## 构建与验证

执行主机均为 `build-test`，Rust1.98.0，编译2任务、禁增量、dev/test debug=0；源码从 Mac 同步到 `/srv/build/tokengo-p1-batch3-20261027/source`，267个输入文件清单 SHA256：`eb59788071905e493ed65563b6bef4dfa4072a14df2b7f25b207fb65039f6dd2`。无真实 .env、Key/master、数据库或 Mac 依赖复制。

组合验收唯一计数：服务端核心库542通过、2忽略；HTTP/集成30目标253通过、1忽略；管理端156通过。合计951通过，3个忽略不计成功。核心库首轮541通过/1失败（测试直接查询未归一化别名），修正测试为现有 normalize_model_name 后 W08两项通过；仅测试 SQL 变化，其余541项有效结果复用。不是声称某次完整命令直接输出542/0，也不为修正测试材料重复全套。30个HTTP目标由新增2目标12通过与收尾28目标241通过组成，不重复计数。Web高级配置先以有效声明复现未知字段拒绝，再修复并运行156项全量管理端逻辑检查。

3个忽略分别为两个6万行 release 性能基准和 real_registry_fetch 外部注册表检查。没有将这些忽略说成通过，也未运行默认并行960请求压测或完整工作区发行CI。原有 dead_code 警告保留；没有修改业务期限或断言来隐藏产品失败。

Linux服务程序与 Web wasm32-unknown-unknown 编译成功，最终真实用例 no-run 成功。程序 SHA256：`f1af1ebb03a9ade696701ca1085ab55b1f66544db0fc33c393dd58c34c65c607`；原始WASM编译件：`7bf2ed757c40afa87f354d3e5b24c830b0eaa2c24a88c2a8bb6327cbbfd8de48`；真实测试程序：`cbf8a22d286ad972d0d01e4ab7bea5d9ff1c225e5d3feb75cad34130c1504b2a`。它们是未优化验证件，WASM尚非Trunk发行bundle，不代表浏览器渲染或正式发行验收。构建命令、目录、退出码、日志和产物均回收Mac `root/logs/` 与 `root/artifacts/`。

失败轮次均保留：共享库新字段漏补的旧测试构造器已机械加None；旧usage精确预期增加本批要求保留的raw观测；测试model/strict schema/outcome预期修正均对应真实协议与既有结算规则，未调整生产账务算法。

官方形态回归使用合成 loopback 上游，覆盖工具多轮、逐字节 Unicode SSE、重复/累计相似思考片段按增量原值追加、截断、错误顺序、neutral metadata、敏感空 choices、双来源重试保护、原生 passthrough。账务断言核对每个实际尝试唯一 ledger/settlement、预留归零、budget spent 与分配一致；输入7/输出9、reasoning4的样本只扣16，不再加4。成功 JSON 与流式的既有 outcome 名称不同，测试按每个请求精确断言，不改产品结算字段迁就测试。

硅基流动托管 `Qwen/Qwen3.8-27B` 最终 **8/8真实调用成功**，全部HTTP200、无请求错误：Chat/Responses 各JSON、SSE、工具首轮及工具结果续轮，验证固定文本、名称、精确参数、call id、结果关联、usage及流式终态。developer原请求保留，Chat strict=false、Responses省略strict的转换检查保留。合计输入1569、输出117 tokens，均为合成材料，价格是本地测试准入配置。

运行器绑定上述源码清单与测试二进制SHA256，退出0。单个SF Key仅经内存/stdin进入已编译进程环境，不经过Cargo、不落明文文件；临时目录0700，数据库/master0600，退出后已删除并核查残留为0。`live/evidence/live-result.json`、`execution.json`及日志已回收Mac；未自动重试，未修改生产/客户端配置。

硅基流动托管结果不能替代 Kimi/GLM/MiniMax 官方账号、百炼工作区、实际 Ollama/vLLM 部署的验收；原文档、模拟和真实调用分别保留。有效工具输出不证明供应商执行了 strict schema。本批不改真实价格、不校验供应商账单，不把未知 usage 当免费。

## 交付、回滚与后续

证据修订 `2026-10-27-p1.2` 仅描述合同，不授予能力或改变历史代次。无数据库迁移、不清空会话或更换 Key。高级身份字段变化使用已有代次隔离，旧默认记录保留。旧程序的严格反序列化不认识非空 runtime_identity：若未来启用该字段，降级前须备份配置、在新程序中移除受影响来源该字段并保存，或保留可读取新字段的程序，不能直接假定旧程序能读新配置；该变更会使相关代次/手工默认需重新确认。未来部署前保留应用和配置备份，回滚不覆盖新账本，不删除签名求兼容；停用旧程序不能承载的新转换路径。

Mac 任务证据：仓库同级 `review/provider-p1-batch3-20261027/`，`w07/w08/w09` 为资料与组件记录，`root/logs` 为命令、失败及退出码，`root/artifacts` 为验证产物，`live/evidence` 为真实调用脱敏结果。早期编译错误、测试材料缺 model/schema 和结算 outcome 预期修正均留证，不用最终成功覆盖失败。

后续 W10–W12 继续诊断、独立采集、覆盖汇总及发行验收；原始研究台账保留研究时状态。本批提交本地 Git，不代表已推送、发行、部署，也不把尚未证明的源能力解除保护。
