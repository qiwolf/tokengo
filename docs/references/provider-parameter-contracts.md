# 模型家族、认证线路与参数匹配

状态：2026-10-25 本地实现稿。尚未编译、运行测试或进行上游验收，不属于已发布能力声明。

## 选择顺序

添加凭据时先选模型家族，再选择认证线路。GPT、Claude、Gemini、DeepSeek、Qwen、Kimi、GLM、MiniMax、Doubao、MiMo 或其他/多家族是用户选择提示；同一 API Key 可以使用多个家族。这个提示不写入持久能力声明，也不删减模型目录。

实际执行规则由供应方端点或管理员显式 parameter_profile、真实凭据绑定、协议、具体模型、API 版本及对应目录共同确定。API 与订阅 OAuth 不共用能力事实。OpenAI / Codex 沿用既有独立账号授权。新设备授权实现覆盖 Kimi Code、MiniMax 国内/Global 和自有 GitHub OAuth App；须配置本部署独立客户端标识后才能开始。认证成功与推理可用分别判定，Copilot 尚未开放未确证的原始推理传输。详见 [账号授权来源](../oauth-providers.md)。不能填写 API Key 冒充订阅授权。

## 参数处理

Chat 与 Responses 增加双向结构化格式、verbosity 和同语义身份/缓存字段转换。JSON Schema 的 name、description、strict、schema 保留；无法表示的字段明确拒绝。缓存 TTL 与 retention 不是别名，可以独立存在。

推理强度 levels 与 thinking_modes 分开记录。上游明确声明的档位原值保留；只有有证据的供应方、模型和协议组合才做原生开关映射。none 不自动换成 low、minimal 或 between_tools。原生未知枚举保留不能算已确认支持；跨协议无法等价表达时拒绝。OAuth 的档位依据来自真实绑定目录或明确的人工配置，不能用公共 API 的模型枚举代替订阅目录。Kimi 目前仅开放目录确认的原生 Chat 型号；MiniMax 需手工填写核实的型号和边界。

输出 cap 按语义区分总输出与仅答案 token。显式上限不再通过学习重试悄悄缩小。推理预算、候选数量和来源已验证边界参与预占；无法确定推理预算时需要保守来源上界。价格表不充当上游能力证明。原生 Anthropic 非流式 max_tokens=0 缓存预热单独处理，要求空内容、正确结束原因和合法输入用量；不放宽普通空响应或流式零输出路径。

配置 parameter_profile 支持 openai、anthropic、gemini、deepseek、dashscope、moonshot、zai、minimax、ark、siliconflow、mimo、ollama、vllm、native。代理或自建地址应明确选择合同，不能由端口或模型名称猜测。native 表示不套用这些新增供应方规则，既有授权、协议、状态、计费和媒体门禁仍然执行。OpenAI managed OAuth 不允许借用其他 API profile；新外部 OAuth 使用其固定来源协议，不套用 OpenAI 的强制 Responses/SSE 处理。

这些适配规则与默认识别随源码分发，不依赖生产数据库手工补丁。新字段均有兼容旧配置的默认值；未选择 parameter_profile 时省略序列化，避免无故改变旧来源指纹。新增来源配置仍需用户填写真实认证和已验证边界，安装镜像或二进制不会获得任何订阅授权。可空输出 cap 的 null 按省略处理，仍执行默认预算，不能借此取消预占。

## 覆盖与边界

逐项清单位于 crates/server/src/parameter_contracts_inventory.json；管理接口仅返回一次完整清单，各模型返回有界来源摘要。清单区分基线转换、当前转换和执行器门禁，不能将原生字段保留解释为无条件可执行。

Gemini 原生 generateContent 传输尚未新增。托管工具、会话状态、多模态附加计费、响应 refusal/annotations/logprobs 等继续受既有执行器与协议边界约束。Ollama/vLLM 的运行时模型能力需要实际元数据；人工 profile 不是实测。未知未来字段和所有第三方代理不在“全部已适配”的承诺范围内。

## 固定来源

LiteLLM 参考版本为 72f1ac4a2795e160d1044fb963041aec3111ff70，仅参考非 enterprise MIT 部分的分层方法。未引入 Python 运行时，也不采用默认丢参、伪造推理历史或强制工具模拟 JSON。

- [主要供应方资料](parameter-contracts-research-major.md)
- [中国与本地供应方资料](parameter-contracts-research-china.md)
- [LiteLLM 静态审计](parameter-contracts-research-litellm.md)

逐来源资料矩阵：[主要供应方](parameter-contracts-matrix-major.json)、[中国与本地供应方](parameter-contracts-matrix-china.json)。

## 后续验收

用户允许进入构建阶段后，按 build-test 流程同步精确工作树，执行项目编译及协议、预算、路由、用量、账务、OAuth、流式回归，取回日志和产物。需要逐认证线路进行真实上游验证；没有账号的线路保持未验证。当前仅完成源码与 JSON/Git 静态审阅，不以已编写用例代称通过。

正式三步接入 UI 与网络代理也已进入本地源码；客户端接入协议按供应方原生声明和转换开关显示可用性。代理配置见 [网络与代理](../outbound-proxy.md)。这些增量均未构建或运行验收。
