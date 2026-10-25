# 已批准模型家族 UI 与 OAuth 实施

基线：main796c78e。实施前所有自有分支已包含、3工作树干净、无stash，证据在主项目 review/production-ui-oauth-20261025/git-gate.json。

用户已批准 review/parameter-ui-20261025/index.html；不再要求重复设计确认。正式代码沿用Rust/Leptos及原有权限/账务路径。客户端接入协议=客户端调用TokenGo，供应方接口协议=TokenGo调用上游。

- 前端：两行品牌Logo选择卡片、更多展开/收起、当前选择可见；API/OAuth独立来源，真实start/poll/cancel/账号选择绑定，保持现有密钥与目录编辑保护。
- 后端：固定供应方设备OAuth授权、加密pending和token、轮询速率/过期/取消/一次完成、刷新租约/CAS、本地撤销与凭据版本。只有可证实的公共客户端配置才启用。
- 集成：外部OAuth不走OpenAI专属Claims/Responses/SSE协议；供应方身份、目录、绑定、上下文/价格及请求透传分别匹配，不能因为有OAuth就强迫OpenAI语义。
- 证据不足或需独立注册的来源明确显示不可用原因；不借用秘密client secret，不生成真实授权、交换用户凭据或调用收费模型。
- 本轮仅源码、文档、未执行回归用例与静态检查。禁止构建、编译、测试、安装依赖、GitHub推送、发布和部署。实际OAuth授权/刷新/模型/用量/账务尚待后续验收。
- 收尾重新检查main及其他工作树，保护会话更新，仅本地提交整合。

用户追加：客户端协议跟随来源原生/无损转换能力；模型家族默认及渠道单独代理。实现支持source>provider>family>系统网络，HTTP/HTTPS、加密认证、直连、CAS、统一授权/刷新/目录/推理出口。
