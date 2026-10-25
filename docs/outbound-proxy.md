# 后台代理策略实现说明

2026-10-25。实现已在 build-test 构建并完成定向回归，私有实例 `e281c51` 已部署；后台入口与代理配置组件已有渲染核对。四条真实设备授权的发起/轮询/取消在隔离环境验证。公开发行与真实用户授权后的推理验收分别见[发行收据](acceptance/2026-10-25-qiwolf18-release/README.md)和[OAuth边界](oauth-providers.md)。

## 配置与范围

后台新增家族默认、渠道（provider_channels）、具体模型接入（channels）三级代理策略。优先级为source > provider > family >现系统网络。每层支持inherit/direct/proxy；inherit继续找下一层，direct明确no_proxy，proxy明确覆盖环境代理。仅HTTP/HTTPS，未加入SOCKS支持或新依赖。

新root-panel API：GET /api/network/proxy-policies；PUT /api/network/proxy-policies/{family|provider|source}/{id}。目标必须存在；family为受控15项枚举。策略存settings，用户名/密码分开以SecretBox密文存储，只读DTO仅返回规范代理URL、has_credentials和版本。URL禁止userinfo、非根路径、query、fragment。认证操作显式keep/replace/clear，更换代理URL时禁止悄悄沿用旧代理认证。CAS防并发覆盖。更新会清空复用客户端缓存。

network_family是明确的网络选择，不承担模型能力声明。优先source和provider已保存值，随后官方精确endpoint识别；不按模型别名或模糊域名猜测。尚未保存新渠道时没有父渠道标识，OAuth初次授权按认证来源使用默认网络家族：OpenAI=gpt、Kimi=kimi、MiniMax=minimax、Copilot=other（其他/多家族）。界面显示并编辑这一实际策略，不能把选择的GPT/Claude模型家族误当成Copilot初次授权出口。保存渠道后可设置其network_family和代理覆盖。

## 实际传输

AppState添加egress Runtime，Upstream用Weak<AppState>访问即时DB策略，避免来源新增后缓存不到父渠道关系。推理call_via、fetch_models、custom_probe均在请求前解析策略，缓存按purpose和有效代理内容散列；不把版本号/家族名当实际代理差异。OpenAI专用TOKENGO_OPENAI_PROXY仅作为inherit基线，显式direct/proxy不读取它；启动不再提前解析该变量阻止已配置策略接管。独立无State的传输fixture保留请求时legacy行为。

OAuth工厂：egress::oauth_client(state, oauth_provider, account_id, origin_provider_id)。初次授权可使用已存在的父渠道；新建尚未保存时传null，按认证来源默认网络家族执行。pending与account密文保存origin_provider_id。刷新、目录、userinfo和撤销依据当前账号绑定来源解析；同账号多个来源的有效策略冲突时拒绝，不悄悄绕行直连。旧OpenAI与新外部设备授权已委派后端接入该统一工厂。

## UI

ProxySettings独立组件挂在正式添加流程的渠道、家族高级设置和参数页的具体接入设置。密码只写不读，保存前清空输入，keep/replace/clear显式；异步读取与保存使用generation及disposed guard，不用旧回包覆盖新CAS版本。不嵌套form，Enter输入不误提交外围接入表单。

## 验证与未验范围

本轮build-test定向构建和测试包括代理URL/密文安全DTO、CAS、改地址时认证处理、source/provider/family策略选择与冲突等代码回归；UI已核对HTTP/HTTPS、继承、直连和首次Copilot授权使用other策略的说明。整体测试与最终发行状态见[发行收据](acceptance/2026-10-25-qiwolf18-release/README.md)，不把测试存在或静态审查代称真实网络连通。

四条设备授权真实发起/轮询/取消只证明该build-test环境的对应路径。各部署自己的代理实际连通、HTTP CONNECT、认证代理、全部OAuth阶段、真实用户刷新/推理、代理凭据轮换和移动端交互仍需按现场配置验收；不能泛称所有网络、所有代理与所有账号已通过。

## 配置入口

添加凭据的第一步打开“网络与代理”，未保存时设置实际认证来源的家族默认，保存后可设置当前渠道覆盖；第三步的“此来源的网络与代理”设置具体模型接入覆盖。对兼容代理或多家族账号明确设置网络家族；认证开始携带已保存的当前渠道身份，尚未保存则使用上述默认家族。未配置的层级继续继承，强制直连会阻断环境代理继承。

代理 URL 与认证分开填写，不在 URL 放用户名/密码；保存后只返回是否已配置认证，不回显原值。选择更换代理地址或直连时，已保存认证须明确选择清除或替换。暂不支持 SOCKS，不能将其当 HTTP 代理保存。
