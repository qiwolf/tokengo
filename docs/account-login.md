# 浏览器账号与接口合同

面板账号与API user/VKey分离。迁移32将旧panel_password_hash原样迁入唯一root，初始用户名admin，不重设旧密码；历史API user不会自动变成登录账号，也不从role/admin_access推导root。普通账号由root明确选择真实启用user_id创建。

浏览器使用 `tokengo_account_session` Cookie，Path=/、HttpOnly、SameSite=Lax、12小时绝对有效期；HTTPS请求设置Secure（依据Origin HTTPS或反代X-Forwarded-Proto=https，仅可加强Secure；当前服务监听HTTP，TLS在前置代理终止；HTTP本地访问不设Secure）。不向JSON/前端暴露session secret，数据库新会话仅存SHA256摘要。新建legacy运维会话也改存带域前缀的摘要，历史明文legacy rows启动时摘要迁移，原Bearer值仍可验证。

所有浏览器unsafe请求（含登录/首次设置，以及旧管理API经任一Cookie认证）必须带 `X-Tokengo-Panel: 1`。Origin若存在必须scheme有效且authority与Host一致，Sec-Fetch-Site为cross-site则拒绝；不开放CORS。Cookie与Authorization同时出现明确401，不尝试fallback为更高权限；无Cookie的旧运维Bearer独立兼容。新账号接口禁止Bearer替代登录Cookie（accounts管理允许root legacy Bearer）。

Account投影为 `{id,username,display_name,role,user_id,enabled,created_at_ms}`，role仅root/user。统一失败 `{error:{message,type}}`：认证无效/错密码/未知账号/禁用统一401；分权/CSRF403；格式/字段400；有界登录节流429。输入/secret/hash不进入错误或审计。

| API | 输入 | 成功输出 |
| --- | --- | --- |
| POST /api/account/login | `{username,password}` | `{account}`，设置Cookie |
| GET /api/account/me | 无 | `{account}` |
| POST /api/account/logout | `{}` | `{ok:true}`，撤当前会话/清Cookie |
| PUT /api/account/password | `{old_password,new_password}` | `{ok:true,reauthenticate:true}`，撤本人全部会话/清Cookie |
| PUT /api/account/profile | `{username,old_password}`，仅本人root | `{ok:true,reauthenticate:true}`，撤全部新旧root会话/清Cookie |
| POST /api/account/setup | `{password,display_name?,gateway_key?}`，仅首次 | `{account,gateway_key,gateway_key_generated}`，设置Cookie；gateway_key仅首次回显，无session token |
| GET /api/accounts | 仅root | `{accounts:[Account]}` |
| POST /api/accounts | `{username,password,display_name,user_id,role?:"user"}` | `{account}`，只建普通账号 |
| PUT /api/accounts/{id} | `{display_name?,enabled?,password?}` | `{account}`，普通账号重置或禁用撤其全部会话；root拒password字段，必须本人安全接口确认当前密码 |

首次状态沿用GET /api/auth/status.needs_setup。新密码12..256字节；迁移旧短密码仍可登录，不能因密码策略锁死现有root。用户名3..64 ASCII字母数字/._-，规范化为小写；改名必须确认当前密码，旧名失效。root保持唯一，不能禁用/修改role或关联user_id，不可从普通账号升级；用户新增要求允许root本人改username，不固定保留admin这个名称。旧 `/api/auth/login` 也必须提供当前root username+password，拒绝password-only避免绕过改名。旧已存Bearer在改名/reset时全部撤销。

root改密码/安全重置与旧global hash在同一事务同步，撤全部new root+legacy sessions；旧set_panel_password也同步root记录，绝不造第二个admin。Argon验证在有界blocking槽中执行，返回后事务复核hash/version；并发setup仅一方成功，login/password/reset/rename之间CAS不接受旧快照。普通账号及其关联user每次鉴权实时检查启用状态；users禁用触发器撤关联会话并增加generation，重新启用不能复活旧Cookie。

普通门户 `GET /api/account/portal?before_id=i64&limit=1..100`：

```json
{"account":{},"keys":[{"id":1,"name":"example","hint":"masked","enabled":true,"revoked_at":null,"expires_at":null,"parent_key_id":null,"created_at_ms":0}],"usage":{"summary":{"request_count":0,"ledger_count":0,"input_tokens":0,"output_tokens":0,"unknown_input_entries":0,"unknown_output_entries":0,"unpriced_entries":0,"known_debit_micro":0},"entries":[{"id":1,"request_id":1,"key_id":1,"model":"example","billing_unit":"token","status":"unpriced","outcome":"ok","input_tokens":null,"output_tokens":null,"usage_certainty":"unpriced","debit_micro":null,"created_ms":0,"settled_ms":0}],"next_before_id":null}}
```

只允许role=user、绑定当前user_id；不接受user_id query覆盖。summary全本人历史，token/known_debit字段是已知项之和，unknown/unpriced计数必须一同显示，不能称缺失为已知0。记录显式白名单，不含source/auth/price snapshots、channel配置/headers、IP、原request/error或任何key明文/hash。普通用户无自助发key接口；看见元数据不等于管理权限。root门户403，应进入管理控制台。

本文件是实现合同，最终验证状态另见本次测试证据与交付报告；不代表已经部署或真实页面验收。
