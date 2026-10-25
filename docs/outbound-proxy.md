# 后台代理策略实现说明

2026-10-25。仅源码静态实现及审阅，未构建、编译、运行测试、打开真实授权、探测外网或部署。

## 配置与范围

后台新增家族默认、渠道（provider_channels）、具体模型接入（channels）三级代理策略。优先级为source > provider > family >现系统网络。每层支持inherit/direct/proxy；inherit继续找下一层，direct明确no_proxy，proxy明确覆盖环境代理。仅HTTP/HTTPS，未加入SOCKS支持或新依赖。

新root-panel API：GET /api/network/proxy-policies；PUT /api/network/proxy-policies/{family|provider|source}/{id}。目标必须存在；family为受控15项枚举。策略存settings，用户名/密码分开以SecretBox密文存储，只读DTO仅返回规范代理URL、has_credentials和版本。URL禁止userinfo、非根路径、query、fragment。认证操作显式keep/replace/clear，更换代理URL时禁止悄悄沿用旧代理认证。CAS防并发覆盖。更新会清空复用客户端缓存。

network_family是明确的网络选择，不承担模型能力声明。优先source和provider已保存值，随后官方精确endpoint识别；不按模型别名或模糊域名猜测。多家族Copilot初次授权须保存父渠道network_family才能使用相应家族默认，否则使用other/系统网络，不伪称自动匹配单一家族。

## 实际传输

AppState添加egress Runtime，Upstream用Weak<AppState>访问即时DB策略，避免来源新增后缓存不到父渠道关系。推理call_via、fetch_models、custom_probe均在请求前解析策略，缓存按purpose和有效代理内容散列；不把版本号/家族名当实际代理差异。OpenAI专用TOKENGO_OPENAI_PROXY仅作为inherit基线，显式direct/proxy不读取它；启动不再提前解析该变量阻止已配置策略接管。独立无State的传输fixture保留请求时legacy行为。

OAuth工厂：egress::oauth_client(state, oauth_provider, account_id, origin_provider_id)。初次授权用父渠道和明确家族；pending与account密文保存origin_provider_id。刷新、目录、userinfo和撤销依据当前账号绑定来源解析；同账号多个来源的有效策略冲突时拒绝，不悄悄绕行直连。旧OpenAI与新外部设备授权已委派后端接入该统一工厂。

## UI

ProxySettings独立组件挂在正式添加流程的渠道、家族高级设置和参数页的具体接入设置。密码只写不读，保存前清空输入，keep/replace/clear显式；异步读取与保存使用generation及disposed guard，不用旧回包覆盖新CAS版本。不嵌套form，Enter输入不误提交外围接入表单。

## 静态检查与待验收

已写但未运行：非法代理URL不回显凭据、AES密文与安全DTO、CAS、变更代理不得沿用旧认证、source/provider/family优先级、等价直连策略不误判账号冲突、显式策略不读legacy代理环境、OAuth原生探针协议和Messages cap保留。

仍须后续按用户允许的build-test流程验证：编译、root权限、代理实际连通、HTTP CONNECT、认证代理、直连绕过环境、OAuth start/poll/refresh/models/revoke各阶段一致、账号多来源冲突、保存后缓存失效、移动端组件交互。当前不能声称这些运行行为已通过。

## 配置入口

添加凭据的第一步打开“网络与代理”，分别设置当前渠道和当前家族默认；第三步的“此来源的网络与代理”设置具体模型接入覆盖。对兼容代理或多家族账号先明确保存网络家族，认证开始携带当前渠道身份。未配置的层级继续继承，强制直连会阻断环境代理继承。

代理 URL 与认证分开填写，不在 URL 放用户名/密码；保存后只返回是否已配置认证，不回显原值。选择更换代理地址或直连时，已保存认证须明确选择清除或替换。暂不支持 SOCKS，不能将其当 HTTP 代理保存。
