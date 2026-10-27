# W11版本化验收索引

研究快照 `gap-ledger.json`、`sources.json`、`issues.json`保留原时点状态，不用修改历史JSON来宣布后续完成。当前核心20项逐条见[核心审计表](../../acceptance/2026-10-27-provider-core-audit.md)。

|材料|作用与边界|
|---|---|
|[来源矩阵](w11-source-matrix.json)|每行独立vendor、鉴权线路、精确模型、原生/客户端协议、API/schema与SDK证据状态；测试标签不证明账号权限或滚动模型修订。|
|[问题状态观测](w11-issue-observations.json)|2026-10-27从16个GitHub官方API重新读取state/closed_at/merged_at；关闭或合并不等于修复发行，也不等于TokenGo已复现。|
|`crates/server/tests/fixtures/provider_compatibility/v1/manifest.json`|9种合成SSE输入及逐项期望；来源明确synthetic_contract_shape，不能冒称真实供应商抓包。|
|`crates/server/tests/provider_compatibility.rs`|生产转换器按1/7/完整块处理，比较文字、工具身份/参数、终态及usage；成功6样本导出生产Responses/Chat JSON+SSE供SDK消费。|
|`crates/server/tests/fixtures/provider_compatibility/sdk-lock.json`|PyPI官方发布元数据锁定openai-python3.40.0及wheel SHA256/声明依赖；原始声明不冒称已安装依赖锁。|
|`crates/server/tests/fixtures/provider_compatibility/sdk_compare.py`|固定SDK版本，loopback HTTP对同一生产转换器输出做raw HTTP与SDK比较；两协议×JSON/SSE×6样本共24对比。实际依赖manifest随结果回收。|

SDK官方发布元数据：[PyPI 3.40.0](https://pypi.org/pypi/openai/3.40.0/json)。wheel SHA256 `dfcfd4f65ee5a8bb824da90c9ad7fa0dda7f3bd472a7c564dffc6183f0a0b55c`，具体文件URL与发布时间在sdk-lock中。此SDK测试只证明所安装版本消费合成输出的行为，不代表Gemini、Anthropic等其他SDK已经比较，也不把issue记录的旧SDK版本当本轮运行版本。

已有HTTP负例直接复用 `chat_opaque_ownership`、`source_reasoning_replay`、`protocol_conversion_faults`、`provider_parameter_combinations`、`provider_usage_metadata`、`minimax_split_gateway`，覆盖归属隔离、参数拒绝、提交前/后失败、取消与计量相关行为；新增形态入口不重复构造这些数据库/路由夹具。真实SF测试继续使用受控、默认忽略的 `provider_live_siliconflow`，结果必须绑定最终源码；此前8call仅作历史记录。

构建和SDK执行由本批root在build-test完成，日志与产物取回仓库同级 `review/provider-p1-batch4-20261027/`。本索引本身不是执行成功凭据。

2026-10-28补充：[SF文档与账号目录漂移](w11-siliconflow-supplement.md)记录官方HTML hash及单账号98模型catalog摘要。新增目录行仅observed_catalog，三family预算/文档范围未闭合不执行8call；最终Qwen候选fresh验收单列。整页HTML和账号原始记录只留本地，不公开上传。

Messages中性追踪的后续兼容见[trace DTO与SDK边界](w11-messages-trace.md)。固定anthropic-python2.3.0的容忍测试消费Rust生产转换器实际导出结果；JSON extra与SSE event extra分别检查，不冒称helper最终快照合并晚trace。

Native缺少硬上界证据的最新可用策略见[本地预占估算](w11-native-reservation-estimates.md)：source/Price/既有4096默认政策分别标来源，不把本地估计写成供应方约束，也不删除真实授权与结算。
