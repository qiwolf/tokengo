# Model limit discovery focused review

2026-11-14。审查当前 Mac 工作树中的 discovery backend、gateway 接入与 Web 自动填入。方法为源码/差异检查；未重复运行已通过测试，未把静态审查当作浏览器或真实供应方验收。

## 结论

主流程符合“系统发现后预填、管理员可调整”：价格保存显式值优先，遗漏才自动回退；来源执行上限独立于价格，优先 OAuth/已保存协议参数，其次同来源模型的有效供应方声明。未发现探测自动覆盖现存价格行、放大已保存源窗口、凭据跨来源复用或把未知值猜成数值的路径。

发现两项局部问题：一项 Web 编辑草稿清空已修复；一项排队期间目录输入上限变大的 TPM 预占漂移也已按主代理授权修复。两处均等待主代理完成本轮增量行为验证；本次静态审查未再发现未解决的实现阻断。

## 已核对

- `routes/billing.rs`：显式管理员输入/输出窗口先于 metadata；明确 null/非整数不静默回退；已知非生成输出须为0。现存价格修改仍生成新版本，不更新原财务行。
- `routes/model_limits.rs`：root 面板鉴权；复用来源自己的模型目录传输、Key、头和代理；不拿 API Key 探测 OAuth。正整数声明逐字段解析，不从型号名或描述猜窗口。响应明确标记 `supplier_declared` / `not_declared`，不是付费大输入硬窗口实测。
- discovery cache：键按 source+logical model 隔离；读取及返回 provenance 均校验来源 fingerprint、binding generation 和上游模型；网络期间不持数据库锁，写回再次核对身份/映射；缓存与审计同 savepoint。
- `pricing_options.rs`：有效缓存只补缺；返回同身份校验的 URL、时间和声明状态。手工来源/OAuth与模型限制仍优先；未知字段保持 null。
- gateway admission、图片/opaque预占及 per-attempt context 均调用同一有效 context helper；不从价格 cap 反推出上游窗口。账本 source snapshot 区分 `administrator_or_oauth`、`supplier_model_catalog`、`unknown`。既有 pin 限源流程保留。
- Web：编辑已存价格时设置 dirty 标记；自动探测不覆盖此类已有值或管理员输入。异步请求使用选中 source/model、请求 generation 及编辑 revision 防止晚到响应覆盖。显式“重新探测并填入”仍可应用正数声明，最后保存才创建价格新版本或更新来源参数。

## Findings

### P2 — 未声明结果清空有效价格编辑草稿：已修复

原 `billing.rs` 探测回调在 replace=true 且返回 null 时，会清空 metadata 仍属自动来源的输入/输出，即使编辑器正在显示已有价格上限。现改由 `discovered_price_bound` 处理：未知只清除未被手工修改的旧自动 prefill；已有价格/管理员值保留。补充两项针对已存值、管理员草稿、未知返回和显式正数 refresh 的单测。增量验证由主代理执行；此审查未重复测试。

### P2 — 排队期间 discovery 输入上限升高，普通文本可能比 admission 预占更多：已修复，待增量验证

admission 与 per-attempt 动态读取 cache；更新 discovery 不改变 binding generation。若只有该候选、旧声明1M、新声明2M、JSON输入字节上界1.5M、管理员价格上限足够，则 admission input=1M，per-attempt input=1.5M。现有 `input > admitted_input` 仅在 full_context_input（图片/opaque/audio）下检查；普通 generation 文本没有对应检查，TPM 可低预占。输出已有相应漂移检查。

建议对普通 generation 也保留“执行输入预占不得大于本次 admission”不变量，或为请求固定同一个 discovery 快照；不需要改成猜测 tokenizer。该问题已通过协作消息即时告知主代理。


处理结果：gateway 已记录 admission 使用过的 `(source_id, logical_model) -> supplier input window` 小型快照。仅在普通 generation 的该窗口变化或失效时，复用现有 `source_context_changed` 检查，防止执行输入预占大于本次已入场的输入预占。来源声明未变化时，不新增适配前后JSON字节差异的误拒；non-generation batch 不走此新增检查。新增 `queued_discovery_input_window_growth_cannot_exceed_the_admitted_text_reservation` 受控回归：真实来源门口排队、观察quota记录后从1000重探至10000，预期明确拒绝且无推理HTTP调用。测试由主代理在build-test执行，此文件不冒称已通过。
