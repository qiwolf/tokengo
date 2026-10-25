# 全参数来源合同与匹配计划

目标：覆盖常见提供方当前官方参数清单，对每项明确原生保留、可无损映射、来源限定、需原生入口或拒绝原因；不把参数名相似当作语义等价，不承诺未知模型或未来字段已验证。

基线：本地main e7edfb3，已包含另一会话baf2e47和所有现有自有分支；两个原工作树干净、无stash。隔离开发分支codex/provider-parameter-contracts-20261025。

## 当前授权限制

- 本轮只进行本地源码、文档、用例、静态检查和Git整合。不执行构建、Rust测试、依赖安装、付费上游调用、GitHub推送、发布或部署。
- 最终整合前重新读取main、全部分支及工作树；另一会话的更新必须审阅后合并，不能reset/覆盖/盲目stage其未提交改动。
- LiteLLM固定源码版本参考，不引入Python运行时、enterprise代码、默认drop_params、强制工具仿真JSON或补造推理/签名。

## 文件与职责

1. parameter_contracts.rs及inventory JSON：官方来源识别、管理员显式合同、API版本/精确型号参数验证、逐参数清单与匹配说明；未知原生扩展保留，未知能力不伪造。
2. parameter_contracts_china.rs：中国来源差异、精确关闭/冲突规则、输出上限是否含推理的语义、原生额外参数和协议默认差异说明。
3. protocol/request.rs及对应协议用例：双向格式/verbosity/同语义标识和缓存控制；不表示的参数保留明确拒绝，尤其opaque状态、多模态计费、托管工具、logprobs响应语义不可只补请求。
4. protocol_config.rs：可选parameter_profile，None序列化省略以保留旧来源指纹；通过既有管理员配置API选择代理/自建来源合同，手工声明不是实测结论。
5. gateway.rs：接入来源合同，禁止静默收窄客户端明确输出上限，保护计费/状态关键字段不被drop_fields擦除；source-specific推理预算参与TPM和账务保守预占，未知边界明确拒绝或要求配置，不把成本补零。
6. admin DTO：返回当前来源的合同和参数清单，让未知、需要原生入口、拒绝原因可见；不新增绕过授权的执行入口。
7. docs及矩阵：固定官方SDK/LiteLLM来源、参数覆盖表、例外、只静态验证的边界与后续验收命令。

## 参数分类

输入/角色/多模态；输出cap与推理预算；effort/thinking/历史回放；采样/seed/penalties/logit_bias；工具/tool_choice/并行/strict；json_schema/verbosity/logprobs；stream/usage；cache/TTL/身份；service_tier/延迟；会话/持久化/opaque状态；托管工具/安全/额外计费。未知未来字段不能自动宣称完成转换。

## 静态验收与待执行用例

- 编写覆盖原生保留、双向映射、显式null、冲突、未知nested字段、来源/型号/版本、manual合同、答案token与总token不同义、预算溢出、warm-cache零输出、stream与usage、错误与fallback、并发/账务/会话权限的回归用例。
- 本轮只解析JSON/Markdown路径与Git差异，记录未执行构建/测试。构建测试须待用户下一步授权，不能把用例已编写说成通过。
