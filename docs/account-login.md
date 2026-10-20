# 浏览器账号与接口合同

主流程只有一个“用户”入口：管理员一次创建普通用户、登录凭证、授权及可选费用上限；历史用户可以在原详情内补开登录，普通用户登录后自助管理本人的 API Key。内部保留稳定 users.id 与 panel_accounts 凭证分表，用户无须手工关联两套身份。迁移32将旧panel_password_hash原样迁入唯一root，初始用户名admin，不重设旧密码；root仍是独立安全身份，不从API role/admin_access推导。

第一阶段不新增数据库schema，不提供用户到期、自助开关或可配置活动Key上限；浏览器自助创建固定最多32把活动Key。金额上限以整数microUSD表示，不能称为Token套餐。实现及验收状态见[需求覆盖](requirements-status.md)和[验收状态](acceptance-status.md)。

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
| GET /api/accounts（旧版兼容） | 仅root | `{accounts:[Account]}` |
| POST /api/accounts（旧版兼容） | `{username,password,display_name,user_id,role?:"user"}` | `{account}`，只建普通账号 |
| PUT /api/accounts/{id}（旧版兼容） | `{display_name?,enabled?,password?}` | `{account}`，普通账号重置或禁用撤其全部会话；root拒password字段，必须本人安全接口确认当前密码 |

首次状态沿用GET /api/auth/status.needs_setup。新密码12..256字节；迁移旧短密码仍可登录，不能因密码策略锁死现有root。用户名3..64 ASCII字母数字/._-，规范化为小写；改名必须确认当前密码，旧名失效。root保持唯一，不能禁用/修改role或关联user_id，不可从普通账号升级；用户新增要求允许root本人改username，不固定保留admin这个名称。旧 `/api/auth/login` 也必须提供当前root username+password，拒绝password-only避免绕过改名。旧已存Bearer在改名/reset时全部撤销。

root改密码/安全重置与旧global hash在同一事务同步，撤全部new root+legacy sessions；旧set_panel_password也同步root记录，绝不造第二个admin。Argon验证在有界blocking槽中执行，返回后事务复核hash/version；并发setup仅一方成功，login/password/reset/rename之间CAS不接受旧快照。普通账号及其关联user每次鉴权实时检查启用状态；users禁用触发器撤关联会话并增加generation，重新启用不能复活旧Cookie。

普通门户 `GET /api/account/portal?before_id=i64&limit=1..100`：

```json
{"account":{},"keys":[{"id":1,"name":"example","hint":"masked","enabled":true,"revoked_at":null,"expires_at":null,"parent_key_id":null,"created_at_ms":0}],"usage":{"summary":{"request_count":0,"ledger_count":0,"input_tokens":0,"output_tokens":0,"unknown_input_entries":0,"unknown_output_entries":0,"unpriced_entries":0,"known_debit_micro":0},"entries":[{"id":1,"request_id":1,"key_id":1,"model":"example","billing_unit":"token","status":"unpriced","outcome":"ok","input_tokens":null,"output_tokens":null,"usage_certainty":"unpriced","debit_micro":null,"created_ms":0,"settled_ms":0}],"next_before_id":null}}
```

只允许role=user、绑定当前user_id；不接受user_id query覆盖。summary全本人历史，token/known_debit字段是已知项之和，unknown/unpriced计数必须一同显示，不能称缺失为已知0。记录显式白名单，不含source/auth/price snapshots、channel配置/headers、IP、原request/error或任何key明文/hash。用户通过下述浏览器自助Key接口管理本人密钥；portal仍是用量与安全元数据投影，不返回历史密钥明文。root门户403，应进入管理控制台。

## 统一用户与浏览器自助 Key

| API | 请求/行为 | 成功输出 |
| --- | --- | --- |
| GET /api/managed-users | root管理；统一用户、关联凭证安全元数据、现有金额预算；root单独投影 | `{users,root_accounts}` |
| POST /api/managed-users | `{username,password,display_name,policy?,limit_micro?}`；同一事务创建普通user/凭证/可选金额预算/审计；不接受user_id或root身份 | `{user}` |
| PATCH /api/managed-users/{id} | `{display_name?,enabled?,policy?,password?,limit_micro?}`；保留原ID；更新金额上限不清零spent/reserved；用户停用同步其登录状态 | `{user}` |
| POST /api/managed-users/{id}/login-credential | `{username,password,display_name?}`；仅为尚无凭证的历史用户补登录，不重挂既有账号 | `{user}` |
| GET /api/account/keys | 从普通Cookie会话推导owner，返回本人Key/预算/安全策略/获准可见模型/32把上限 | `{keys,policy,budget,models,max_active_keys}` |
| POST /api/account/keys | `{name,policy?,expires_at?,limit_micro?}`；顶级Key归属当前user，不要求先有VKey；事务内检查活动Key数 | `{key,secret}`，secret仅本次返回 |
| PATCH /api/account/keys/{id} | `{name?,policy?,enabled?,expires_at?,limit_micro?,revoke?}`；只能收紧本人Key，停用或撤销，不能自助重新启用 | `{key}` |
| DELETE /api/account/keys/{id} | 撤销本人Key；非本人404；已撤销不能复活 | `{key}` |

统一管理入口使用root授权，保留现有root运维Bearer兼容；普通自助Key接口只接受浏览器会话，Cookie写操作沿用CSRF门，Cookie/Bearer混合拒绝。所有新响应加no-store。模型选项不泄露未授权来源配置或凭据；数据库新Key只存安全摘要，列表永不返回secret。

自助策略必须不宽于实时用户策略，管理能力强制关闭；编辑既有Key还须不宽于其旧策略和父链。金额上限不能高于用户、既有Key或父链上限，不能清空有限上限来逃限；多个Key仍共享用户总额。Key到期可缩短，不能超出既有或父链期限。密码修改/重置撤登录会话，保留API Key与财务身份；用户整体停用阻断登录和新API请求。

历史多个登录凭证共享一个user的记录保留，不自动合并或改账本归属；统一界面显示历史状态。此类用户不能通过统一入口批量重置所有密码，需兼容维护。旧 `/api/users`、`/api/accounts` 与持VKey的 `/v1/keys` 保留兼容，不是普通用户创建/登录的主界面流程。

用户/Key策略改为点选控件，保留null继承、空集合拒绝全部和旧隐藏值；未知字段阻止有损保存。第一阶段未实现policy版本号/409乐观并发冲突，不能声称已有跨页面旧值覆盖保护；请求时实时权限再验证继续生效。

本文件是当前实现合同。定向服务端与Web检查已通过，浏览器闭环、完整最终CI和生产发布按最终提交分别验收，不能从代码或旧版本部署推定已上线。

## 当前授权选择与用户删除

新授权候选仅列可见模型；管理员使用可见模型目录，普通用户还须满足父级真实来源权限。已有隐藏或目录外显式授权放在“已有兼容授权”折叠区，只能撤销；保存其他资料不会丢弃原值，取消不会提交撤销。目录失败时不把全部来源模型作为备选。空集合仍表示禁止全部，与 null 继承不同。

管理员可删除普通用户：`DELETE /api/managed-users/{id}` 归档稳定用户身份，撤销其全部 Key，移除登录凭证/会话和私有策略，保留请求、用量、账本及删除审计。存在该用户运行请求、预占或未结算项时拒绝；根管理员不能从普通用户删除入口移除。普通用户可恢复自己暂停的 Key，但不能恢复管理员停用、撤销或祖先禁用的 Key。
