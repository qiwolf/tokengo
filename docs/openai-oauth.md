# 独立 OpenAI OAuth 注册与会话

此实现使用官方开放源代码应用的 Sign in with ChatGPT 直接流程，访问 `https://api.openai.com/v1`；不复用 LiteLLM 或 Codex 原有 refresh token，不使用 ChatGPT backend-api。账号注册与 token 更新由 Tokengo 服务端统一负责。本机工具只监听127.0.0.1并转交授权回调；尚未完成实际用户授权、账户模型目录与真实推理之前，不能声明订阅迁移已验收。

## 管理接口

全部接口要求根管理面板会话 `PanelAuth`。普通/管理员 VKey 均不能调用，不建立无认证 LAN callback。

- `POST /api/oauth/start`：`{label,redirect_uri,account_id?}`；redirect必须`http://127.0.0.1:<port>/auth/callback`。返回`attempt_id/authorization_url/expires_at/host_id`。本机工具必须先监听再打开浏览器；authorization_url不得打印或落普通日志，返回账号的URL可能含ID token hint。
- `POST /api/oauth/complete`：`{attempt_id,state,code?,client_id?,error?}`。state十分钟有效、一次消费；拒绝错误state；验证正确state的denied回调消费attempt并400，不换token。初始注册必须使用callback签发client ID，后续签发ID不得替换选中账号ID。结果只有账号元数据。
- `GET /api/oauth/accounts`：账号状态/身份/到期/版本和scope，无token。
- `GET /api/oauth/accounts/{id}/models`：用该账号的新鲜token访问官方模型目录，返回最大1MiB/4096项目录，保留来源提供的上下文信息，不暴露token；目录缺上界时必须保留未知，不能凭模型名猜测。
- `GET/PUT/DELETE /api/oauth/channels/{id}`：查询、绑定、解绑。PUT为`{account_id,reasoning_effort?,service_tier?,max_input_tokens?,max_output_tokens?}`。effort限low/medium/high，tier限priority/auto/default。独立context bounds为1..16777216；只配置完整pair时`context_bounds`返回已配置边界。
- `POST /api/oauth/accounts/{id}/revoke`：立即停用并清除本地token，增加所绑定渠道generation；按discovery的官方revocation endpoint尝试终止远程会话，响应明确`remote_revocation_confirmed`。失败时只能确认本地退出，需到ChatGPT设置断开应用；账号/client/host映射保留。

绑定要求来源base为官方API，无旧channel secret或自定义headers；不自动删除旧凭据。绑定设原生Responses，可允许已有文本/函数协议转换；不代表任何请求语义都能转换。更换账号、修改defaults/bounds或解绑使来源generation改变；同账号access/refresh轮转只改credential revision，不改变来源身份。一个账号可绑定多个渠道，仍共享同一会话及刷新锁。

## 凭据与并发

稳定host UUID存于数据库。每次授权独立state、nonce、PKCE verifier；pending payload及整套access/refresh/ID tokens经SecretBox加密。ID token使用官方JWKS验签，限定RS256、issuer、issued-client audience、exp/iat/sub和授权nonce。返回账号必须与选中注册的验证sub一致，不能按email合并账号。续期若返回ID token，同样验证签名、issuer/audience/expiry与原sub；不得换号。

同账号进程内锁与数据库refresh-owner/version CAS共同防止多channel及多进程同时轮换。新token、scope、到期、earliest_refresh_at和版本一起提交；失败保留旧pair，invalid_grant要求重新授权。调用方取消不会取消独立刷新事务worker，以免provider已经轮换而本地未保存。撤销会使在途刷新CAS失败，不能把账号复活。进程强杀/机器崩溃仍可能打断“provider已轮换、本地未提交”的跨系统窗口；不能声称数据库事务能消除该外部窗口，必要时重新授权。

Token response的`earliest_refresh_at`按Unix秒解析，内部时间统一毫秒；到期且未到允许刷新时间时fail closed。未过期但到期前60秒会按允许时间刷新。无有效身份、已撤销、刷新失败或不一致绑定都不提供bearer。OAuthCredential不实现Debug/Serialize，管理错误为固定机器码，不复制token endpoint错误正文。

## 当前官方预览边界

官方HTTP必须stream:true、store:false、完整input数组；不能用previous_response_id恢复远端状态。显式system item不被支持，应由调用方采用instructions/developer，不默默降级优先级。

预览不接受max_output_tokens、temperature、top_p、background、conversation、max_tool_calls、metadata、moderation、multi_agent、prompt、prompt_cache_retention、safety_identifier、top_logprobs、truncation、user等字段。网关应明确拒绝不可表示字段，不能为通过请求静默丢掉输出cap。此OAuth来源无法通过max_output_tokens强制限制输出；独立max_input_tokens/max_output_tokens配置必须是已核实的完整provider上下文/输出上界，用于本地admission与财务保守预占，不能假装是已发送并被provider执行的cap，不能因此虚构零价格。订阅成本未知仍保持未定价。

函数/custom工具按官方要求组织namespace或additional_tools；不支持音频/视频、Files upload/transcription、image generation、file search、Code Interpreter、native computer、hosted MCP/connectors、tool_search等。原生Responses实现不代表完整Chat或Messages客户端能力；具体转换与CLI验收必须分别进行。

## 验证边界

回归使用独立合成RSA测试密钥、loopback JWKS/token/revocation/catalog服务；没有调用真实授权或读旧CLI认证。测试覆盖非法JWT、一次性state、跨账号、并发轮转/跨runtime CAS、刷新取消/撤销、重启保留、密文存储和管理权限。测试用api_base覆盖仅在cfg(test)编译；生产端点固定，不能经API/环境配置任意host。

官方依据：[注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)、[账号与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)、[token字段](https://developers.openai.com/siwc/token-sharing-open-source/token-reference)、[预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)。


订阅传输会将顶层裸 function/custom 工具原样移到首个 developer `additional_tools` 输入项，保留名称、schema、调用和返回 ID；已有 namespace 不改。此适配遵循[官方 additional_tools 结构](https://developers.openai.com/api/docs/guides/tools-tool-search)。显式 API 不支持字段在发出请求前拒绝；非流式客户端的完整响应从有界 SSE 收集器重建，保留终端空 output 前的 finalized items。OAuth 与独立按次工具禁止旧的无账务自动探活/视觉测试，模型目录走账号专用管理接口，推理走正常 VKey 链路。
