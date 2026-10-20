# 账号密码登录与控制台改善实施计划

目标：以真实账号密码替代单一全局口令的用户体验，管理员和普通用户分权；自动浏览器会话不要求填写token。Rust/Axum/SQLite/Leptos继续使用，保留模型、OAuth、VKey、价格和账本。

用户已选择refactoring-ui清爽专业控制台，结合dsh-ui-craft表单与无障碍约束。本计划采用既有实施授权，不重复索要设计审批。

## 账号与接口约定

追加迁移32及独立accounts/session模块。面板登录账号与API用户、API VKey明确分开；root账号初始admin从现有管理员口令哈希迁入，不重置原密码、不猜测用户显示名到ID。普通账号由管理员明确选择现有user_id创建，无公开注册，无API用户角色到root权限的隐式升级。

新浏览器接口：POST /api/account/login {username,password}、GET /api/account/me、POST /api/account/logout、PUT /api/account/password {old_password,new_password}。成功返回 {account:{id,username,display_name,role,user_id}}，role仅root/user；root不绑定普通用户。登录/登出设置/清除HttpOnly SameSite cookie，不向前端暴露session secret；原有运维Bearer接口保留兼容。浏览器新接口对跨站写入有CSRF防护，后端每次复核账号与关联user启用状态。

root管理接口 GET/POST /api/accounts；PUT /api/accounts/{id} 管理启用状态/显示名及安全重置密码（修改字段与原子语义由后端统一实现并通知前端）。创建普通账号必须有真实user_id；root账户不得从普通role/user权限推导。

普通用户 GET /api/account/portal 返回 {account,keys,usage}，只允许其关联user的数据；keys不得包含任何密钥明文/哈希。usage只暴露本人汇总和限定记录，具体字段前后端同步。普通用户不能访问渠道、模型路由、其他身份或全部账本；不能只靠隐藏导航实现权限。

保留旧管理员密码，迁移后账号admin+原面板密码登录；新设/重置密码使用强哈希及合理长度规则。新root密码变更与旧口令记录同步并撤销全部新旧root会话，避免两个密码源漂移。普通账号修改密码/禁用后旧会话失效。会话库仅存摘要，登录失败不枚举账号，有界验证/失败节流。

## 实施与验收

1. 后端先HTTP红测：账号密码、迁移、错误/禁用、session刷新与撤销、root/用户越权、CSRF、密码变更与旧接口兼容；实现模块及追加迁移，不改既有VKey权限或模型请求路径。
2. 前端采用同源cookie与account/me启动恢复；登录页明确账号/密码、标签/自动填充/可见密码切换/就地错误/加载态。管理员保留现有管理能力，新增面板账号管理；普通用户进入本人门户。登录及Shell统一清爽视觉，桌面/手机无溢出、键盘可用，继续支持深浅色。
3. 独立审查认证边界、迁移与回滚；定向测试与WASM构建，必要完整workspace/发行包检查，不重复已通过且未变部分。
4. 在隔离浏览器验证root与普通账号、错误密码、退出、刷新恢复、禁用/改密；不调用供应商消耗额度。源码提交推送同PR。
5. pve03 CT104完整停服备份后升级，新表迁移仅允许预期差异，业务表/加密授权/账本保持；2c2G读回。同库降级须恢复配套旧快照与旧二进制，不声称新schema直接向后兼容。
6. 真实页面留截图及交付记录，私有访问指南只写本机0600，公开报告不含密码/token/cookie。

## 用户补充：管理员登录账号可修改

管理员本人可在账户设置修改username（必须确认当前密码），不只是显示名。改名后旧username失效，撤销全部新旧root会话并要求重新登录；root权限性质/user_idNULL仍不可编辑。默认admin只是迁移初始值，不是永久限制。当前用户未指定新名字，因此上线后不擅自永久改名。

旧/api/auth/login不可保留password-only绕过改名；运维若使用此兼容入口必须同时提供当前username，或者改用新cookie入口。已有Bearer可在常规升级暂保留，改名/改密必须全部撤销。补充旧名/重复名/未确认密码/普通用户试改root/改名后旧Bearer和cookie失效回归。
