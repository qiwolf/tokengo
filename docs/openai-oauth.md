# 独立 OpenAI OAuth 注册与会话

此实现使用官方开放源代码应用的 Sign in with ChatGPT 直接流程，访问 `https://api.openai.com/v1`；不复用 LiteLLM 或 Codex 原有 refresh token，不使用 ChatGPT backend-api。账号注册与 token 更新由 Tokengo 服务端统一负责。本机工具只监听127.0.0.1并转交授权回调；尚未完成实际用户授权、账户模型目录与真实推理之前，不能声明订阅迁移已验收。

## 管理接口

全部接口要求根管理面板会话 `PanelAuth`。普通/管理员 VKey 均不能调用，不建立无认证 LAN callback。

- `POST /api/oauth/start`：`{label,redirect_uri,account_id?}`；redirect必须`http://127.0.0.1:<port>/auth/callback`。返回`attempt_id/authorization_url/expires_at/host_id`。本机工具必须先监听再打开浏览器；authorization_url不得打印或落普通日志，返回账号的URL可能含ID token hint。
- `POST /api/oauth/complete`：`{attempt_id,state,code?,client_id?,error?}`。state十分钟有效、一次消费；拒绝错误state；验证正确state的denied回调消费attempt并400，不换token。初始注册必须使用callback签发client ID，后续签发ID不得替换选中账号ID。结果只有账号元数据。
- `GET /api/oauth/accounts`：账号状态/身份/到期/版本和scope，无token。
- `GET /api/oauth/accounts/{id}/models`：用该账号的新鲜token访问官方模型目录，返回最大1MiB/4096项目录，保留来源提供的上下文信息，不暴露token；目录缺上界时必须保留未知，不能凭模型名猜测。Sign in with ChatGPT 的正式目录是顶层`models`数组、标识为`slug`，保留全部原始元数据、visibility及顺序；显示选择器时仅展示`visibility=list`，请求模型使用原`slug`。普通API的`object=list/data/id`只作为明确独立兼容形状接受，两形状混合、重复标识、畸形上界或递归凭据字段均拒绝，不把模型目录当成token价格。
- `GET/PUT/DELETE /api/oauth/channels/{id}`：查询、绑定、解绑。PUT为`{account_id,reasoning_effort?,service_tier?,max_input_tokens?,max_output_tokens?}`。此旧连接级 effort 兼容枚举限 low/medium/high，tier 限 priority/auto/default；新模型不使用该旧 effort 枚举选择档位，改用下述每模型原生参数配置。独立context bounds为1..16777216；只配置完整pair时`context_bounds`返回已配置边界。
- `POST /api/oauth/accounts/{id}/revoke`：立即停用并清除本地token，增加所绑定渠道generation；按discovery的官方revocation endpoint尝试终止远程会话，响应明确`remote_revocation_confirmed`。失败时只能确认本地退出，需到ChatGPT设置断开应用；账号/client/host映射保留。

绑定要求来源base为官方API，无旧channel secret或自定义headers；不自动删除旧凭据。绑定设原生Responses，可允许已有文本/函数协议转换；不代表任何请求语义都能转换。更换账号、修改defaults/bounds或解绑使来源generation改变；同账号access/refresh轮转只改credential revision，不改变来源身份。一个账号可绑定多个渠道，仍共享同一会话及刷新锁。

## 新模型的原生推理档位

获取授权账号目录时，会将其明确声明的 `supported_reasoning_levels` / `default_reasoning_level` 等安全能力元数据缓存到仍绑定同一账号及来源身份的连接。缓存不存完整目录正文或凭据，不对同品牌其他连接套用；只声明 reasoning boolean 不会生成 low/high 等档位。新增模型缺缓存时由面板读取目录，手动“刷新能力元数据”同样只读目录。

分发版本内置兼容规则会在**有效且同来源身份的目录**上补齐已核实的遗漏项：官方 OpenAI API/OAuth 的精确 `gpt-6-luna` 增加 `none`。它不是账号目录原始声明，面板标记为“上游声明 + 内置兼容”。目录未知、无效、显式不支持或来源身份已变化时不会据型号猜测能力。读取旧缓存也应用相同规则，无需迁移237的数据库或执行手工SQL；管理员手工列表仍优先，可明确排除 `none`，默认值不自动改成 `none`。普通服务商的关闭语义见[推理关闭兼容](reasoning-compatibility.md)。

首次新建 OAuth Key 必须配置成对、已核实的输入与输出上界。目录刷新不把 `context_window` 当成输出上限，也不自动复制其他账号的边界。旧绑定 API 可以暂存未知/不完整的上界，但此时推理未就绪，会返回 `source_limits_required`；应通过绑定 API 补齐 `max_input_tokens` 与 `max_output_tokens` 后再调用。订阅成本未知仍保持未定价，不补零价。

在服务商模型行展开“连接与推理参数”，即可选择该连接上游明确声明的档位，或保留“使用服务商默认”。`PUT /api/channels/{id}/models/reasoning` 的 `default_effort:null` 明确不注入档位，覆盖旧连接绑定的 effort 默认；客户端显式 `reasoning.effort` / `reasoning_effort` 仍优先。档位值不局限于旧绑定枚举，但必须符合当前目录或明确人工确认的值；元数据变更使已选默认不再有效时要求重新确认，不静默恢复旧默认。

模型名使用当前目录中的标准名称，旧档位别名只作为兼容配置保留。目录声明不证明该档位已做真实推理，也不证明加密/签名语义可跨协议转换；详见[服务商与模型合同](provider-channels.md)及[公开验收状态](acceptance-status.md)。

## 凭据与并发

稳定host UUID存于数据库。每次授权独立state、nonce、PKCE verifier；pending payload及整套access/refresh/ID tokens经SecretBox加密。ID token使用官方JWKS验签，限定RS256、issuer、issued-client audience、exp/iat/sub和授权nonce。返回账号必须与选中注册的验证sub一致，不能按email合并账号。续期若返回ID token，同样验证签名、issuer/audience/expiry与原sub；不得换号。

同账号进程内锁与数据库refresh-owner/version CAS共同防止多channel及多进程同时轮换。新token、scope、到期、earliest_refresh_at和版本一起提交；失败保留旧pair，invalid_grant要求重新授权。调用方取消不会取消独立刷新事务worker，以免provider已经轮换而本地未保存。撤销会使在途刷新CAS失败，不能把账号复活。进程强杀/机器崩溃仍可能打断“provider已轮换、本地未提交”的跨系统窗口；不能声称数据库事务能消除该外部窗口，必要时重新授权。

Token response的`earliest_refresh_at`按Unix秒解析，内部时间统一毫秒；到期且未到允许刷新时间时fail closed。未过期但到期前60秒会按允许时间刷新。无有效身份、已撤销、刷新失败或不一致绑定都不提供bearer。OAuthCredential不实现Debug/Serialize，管理错误为固定机器码，不复制token endpoint错误正文。

## 当前传输与适配边界

官方HTTP必须stream:true、store:false、完整input数组；不能用previous_response_id恢复远端状态。显式system item不被支持，应由调用方采用instructions/developer，不默默降级优先级。

Responses 入口将 max_tokens/max_completion_tokens 明确兼容为 max_output_tokens；同时提供多个上限时必须相等，否则拒绝。它们都表示包含推理在内的输出 token 上限，别名只在入口归一化，不将错误协议字段发送给上游。Chat 入口继续使用原有转换。当前 TokenGo OAuth 适配器在发出请求前拒绝 temperature、top_p、background、conversation、max_tool_calls、metadata、moderation、multi_agent、prompt、prompt_cache_retention、safety_identifier、top_logprobs、truncation、user 等字段，不能为通过请求静默丢掉输出 cap。这是当前适配策略，不应据此断言所有字段都被实时上游拒绝。

2026-10-25 对现有 GPT-6 Luna OAuth 账号的隔离直连探测已确认：max_output_tokens=16/32 会在该 token 数截断，终态为 incomplete、原因 max_output_tokens；32000/64000 被接受；1 返回最低值 16 的参数错误。本次仅核实该账号/模型/接口，不代表所有 OAuth 模型均已覆盖；网关现已支持 max_output_tokens，并校验整数且至少16、不超过绑定的已核实输出上界及计费上界。Chat 的 max_tokens/max_completion_tokens 经转换后应用相同校验，不静默裁剪。显式 cap 用于本地 admission 与账务预占；省略 cap 仍按绑定的完整输出上界保守预占。opaque/视觉输入继续使用完整输入上界；订阅成本未知仍保持未定价。max_output_tokens 截断终态在原生 Responses 中保留 incomplete 和实际 usage，转换到 Chat 时返回 finish_reason=length；其他 incomplete 或错误仍拒绝。

Chat→Responses 转换支持 response_format 的 text、json_object 与 json_schema，映射到 text.format；json_schema 的 name、description、schema 与显式 strict 值保留，省略值不补写。schema 内容不改写，未知格式控制字段或不能转换到目标协议的格式显式拒绝。Chat 原生透传继续保留原请求；原生 Responses 使用 text.format。模型是否接受具体 schema 仍由实际来源决定。

函数/custom工具按官方要求组织namespace或additional_tools；不支持音频/视频、Files upload/transcription、image generation、file search、Code Interpreter、native computer、hosted MCP/connectors、tool_search等。原生Responses实现不代表完整Chat或Messages客户端能力；具体转换与CLI验收必须分别进行。

## 验证边界

可在受限服务环境文件中设置 `TOKENGO_OPENAI_PROXY=http://proxy-host:port`。它只作用于独立的 OpenAI 授权/目录/撤销客户端与订阅推理连接池，其他模型保持原连接池。未设置时沿用原默认网络行为；不需要设置全局 HTTP_PROXY 或修改共享网络。代理配置错误只返回固定错误信息，不打印 URL 或可能包含的代理凭据。

自动化回归使用独立合成RSA测试密钥、loopback JWKS/token/revocation/catalog服务；回归本身没有调用真实授权或读旧CLI认证。另行进行的真实授权、有限客户端与转换验证见[公开验收状态](acceptance-status.md)，不能将合成回归与真实推理结果混为一项。测试覆盖非法JWT、一次性state、跨账号、并发轮转/跨runtime CAS、刷新取消/撤销、重启保留、密文存储和管理权限。测试用api_base覆盖仅在cfg(test)编译；生产端点固定，不能经API/环境配置任意host。

官方依据：[模型目录与推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)、[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)、[账号与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)、[token字段](https://developers.openai.com/siwc/token-sharing-open-source/token-reference)、[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)。


订阅传输会将顶层裸 function/custom 工具原样移到首个 developer `additional_tools` 输入项，保留名称、schema、调用和返回 ID；已有 namespace 不改。此适配遵循[官方 additional_tools 结构](https://developers.openai.com/api/docs/guides/tools-tool-search)。显式 API 不支持字段在发出请求前拒绝；非流式客户端的完整响应从有界 SSE 收集器重建，保留终端空 output 前的 finalized items。OAuth 与独立按次工具禁止旧的无账务自动探活/视觉测试，模型目录走账号专用管理接口，业务推理走正常 VKey 链路；管理员可通过独立[自定义探针](custom-probes.md)执行 OAuth Responses 系统诊断，走正常认证刷新与有界SSE收集，单独记录Token和未知订阅成本，不扣用户额度。


输出 cap 截断时，已观测到同一 reasoning item 的 finalized 事件和最终 output 给出不同 encrypted_content。仅当 item ID、类型和其他所有字段一致且终态明确为 max_output_tokens 时，缓冲器采用最终响应完整原项；不解密、不认定密文等价、不拼接状态。返回和会话归属记录仍使用最终来源原值，其他字段冲突继续拒绝。
