# 浏览器账号与接口合同

主流程只有一个“用户”入口：管理员一次创建普通用户、登录凭证、授权及可选费用上限；历史用户可以在原详情内补开登录，普通用户登录后自助管理本人的 API Key。内部保留稳定 users.id 与 panel_accounts 凭证分表，用户无须手工关联两套身份。迁移32将旧panel_password_hash原样迁入唯一root，初始用户名admin，不重设旧密码；root仍是独立安全身份，不从API role/admin_access推导。

第一阶段不新增数据库schema，不提供用户到期、自助开关或可配置活动Key上限；浏览器自助创建固定最多32把活动Key。金额上限以整数microUSD表示，不能称为Token套餐。实现及验收状态见[需求覆盖](requirements-status.md)和[验收状态](acceptance-status.md)。

浏览器使用 `tokengo_account_session` Cookie，Path=/、HttpOnly、SameSite=Lax、12小时绝对有效期；HTTPS请求设置Secure（依据Origin HTTPS或反代X-Forwarded-Proto=https，仅可加强Secure；当前服务监听HTTP，TLS在前置代理终止；HTTP本地访问不设Secure）。不向JSON/前端暴露session secret，数据库新会话仅存SHA256摘要。新建legacy运维会话也改存带域前缀的摘要，历史明文legacy rows启动时摘要迁移，原Bearer值仍可验证。

所有浏览器unsafe请求（含登录/首次设置，以及旧管理API经任一Cookie认证）必须带 `X-Tokengo-Panel: 1`。Origin若存在必须scheme有效且authority与Host一致，Sec-Fetch-Site为cross-site则拒绝；不开放CORS。Cookie与Authorization同时出现明确401，不尝试fallback为更高权限；无Cookie的旧运维Bearer独立兼容。新账号接口禁止Bearer替代登录Cookie（accounts管理允许root legacy Bearer）。

Account投影为 `{id,username,display_name,role,user_id,enabled,created_at_ms}`，role仅root/user。统一失败 `{error:{message,type}}`：登录凭据无效/错密码/未知账号/禁用统一401；已登录root的安全重置二次验证失败返回403 `step_up_failed`，可在当前会话重试；分权/CSRF403；格式/字段400；有界登录节流429。输入/secret/hash不进入错误或审计。

| API | 输入 | 成功输出 |
| --- | --- | --- |
| POST /api/account/login | `{username,password}` | `{account}`，设置Cookie |
| GET /api/account/me | 无 | `{account}` |
| POST /api/account/logout | `{}` | `{ok:true}`，撤当前会话/清Cookie |
| PUT /api/account/password | `{old_password,new_password}` | `{ok:true,reauthenticate:true}`，撤本人全部会话/清Cookie |
| POST /api/account/security-reset | `{password}`，仅本人root | 撤销全部root新旧会话及有效运维令牌，清Cookie；登录名/密码和业务Key不变 |
| PUT /api/account/profile | `{username,old_password}`，仅本人root | `{ok:true,reauthenticate:true}`，撤全部新旧root会话/清Cookie |
| POST /api/account/setup | `{password,display_name?,gateway_key?}`，仅首次 | `{account,gateway_key,gateway_key_generated}`，设置Cookie；gateway_key仅首次回显，无session token |
| GET /api/accounts（旧版兼容） | 仅root | `{accounts:[Account]}` |
| POST /api/accounts（旧版兼容） | `{username,password,display_name,user_id,role?:"user"}` | `{account}`，只建普通账号 |
| PUT /api/accounts/{id}（旧版兼容） | `{display_name?,enabled?,password?}` | `{account}`，普通账号重置或禁用撤其全部会话；root拒password字段，必须本人安全接口确认当前密码 |

首次状态沿用GET /api/auth/status.needs_setup。新密码12..256字节；迁移旧短密码仍可登录，不能因密码策略锁死现有root。用户名3..64 ASCII字母数字/._-，规范化为小写；改名必须确认当前密码，旧名失效。root保持唯一，不能禁用/修改role或关联user_id，不可从普通账号升级；用户新增要求允许root本人改username，不固定保留admin这个名称。旧 `/api/auth/login` 也必须提供当前root username+password，拒绝password-only避免绕过改名。旧已存Bearer在改名/reset时全部撤销。

root改密码与旧global hash在同一事务同步，撤全部new root+legacy sessions；独立安全重置不改登录名或密码，会撤销全部root会话及有效运维令牌，业务Key不变；旧set_panel_password也同步root记录，绝不造第二个admin。Argon验证在有界blocking槽中执行，返回后事务复核hash/version；并发setup仅一方成功，login/password/reset/rename之间CAS不接受旧快照。普通账号及其关联user每次鉴权实时检查启用状态；users禁用触发器撤关联会话并增加generation，重新启用不能复活旧Cookie。

普通门户 `GET /api/account/portal?before_id=i64&limit=1..100`：

```json
{"account":{},"keys":[{"id":1,"name":"example","hint":"masked","enabled":true,"revoked_at":null,"expires_at":null,"parent_key_id":null,"created_at_ms":0}],"usage":{"summary":{"request_count":0,"ledger_count":0,"input_tokens":0,"output_tokens":0,"unknown_input_entries":0,"unknown_output_entries":0,"unpriced_entries":0,"known_debit_micro":0},"entries":[{"id":1,"request_id":1,"key_id":1,"model":"example","billing_unit":"token","status":"unpriced","outcome":"ok","input_tokens":null,"output_tokens":null,"usage_certainty":"unpriced","debit_micro":null,"created_ms":0,"settled_ms":0}],"next_before_id":null}}
```

只允许role=user、绑定当前user_id；不接受user_id query覆盖。summary全本人历史，token/known_debit字段是已知项之和，unknown/unpriced计数必须一同显示，不能称缺失为已知0。记录显式白名单，不含source/auth/price snapshots、channel配置/headers、IP、原request/error或任何key明文/hash。用户通过下述浏览器自助Key接口管理本人密钥；portal仍是用量与安全元数据投影，不返回历史密钥明文。root门户403，应进入管理控制台。

## 脚本化运维

自 v0.34.0-qiwolf.5 起提供独立运维令牌，推荐部署者在控制台“设置 → 运维令牌”创建后交给可信智能体，详见[运维令牌接口与脚本](operations-tokens.md)。目前只有固定的一档“站点运维”权限，不支持可选 scope；需要渠道级最小权限的 CI 不能据此视为已满足。部署时须核对所用版本与实际能力清单，旧版兼容会话不会自动变为运维令牌。

下面保留旧 root 运维会话的兼容用法，不构成长期稳定性、弃用时间或未来替代方案的承诺。新令牌与旧会话独立管理：普通 root 改名/改密不撤销独立令牌；安全重置或手动撤销会阻止它继续接入。

### 选择凭据

| 凭据 | 当前用途 | 权限边界 |
| --- | --- | --- |
| Gateway Key / 普通 VKey | 获准的推理与自助接口 | 不授予渠道或全局设置管理权限 |
| 管理员 VKey | 用户角色为 admin，且用户、Key 和祖先策略均允许 `admin_access` 时，访问相应管理接口 | 仅覆盖采用 `AdminAuth` 的接口；不是 root。具体接口见[身份与计量](identity-metering.md#api) |
| root 浏览器会话 | 控制台及采用 `PanelAuth` 的管理接口 | HttpOnly Cookie，12小时绝对有效期；写请求需要 CSRF 头 |
| 独立运维令牌 | 部署者的可信智能体访问明确登记的业务管理接口 | 固定站点运维权限；默认90天，最多365天；不能修改root安全或再签发运维令牌 |
| root 运维会话（兼容） | 非浏览器脚本访问采用 `PanelAuth` 的管理接口 | root 权限，不能指定渠道/路由 scope；当前默认30天绝对有效期，不随请求续期 |

渠道、路由及统一用户等业务管理接口支持 root 会话和明确登记的独立运维令牌，具体方法与路径以 `/api/ops/capabilities` 为准；管理员 VKey 不因此获得渠道管理权限。root 安全、运维令牌签发/撤销及未登记动作不向独立运维令牌开放；`GET /api/settings` 仅返回安全投影，完整设置写入仍要求 root。不要把 `admin_access` 或推理来源白名单理解为管理面渠道授权。旧 root 运维会话可能访问敏感配置，应按 root 凭据保管，不向仅需推理或有限管理权限的应用分发。

### 登录、调用与撤销

实例须已完成首次初始化。非浏览器脚本使用 `POST /api/auth/login`，JSON 为 `{"username":"当前root登录名","password":"当前密码"}`，成功返回 `{"ok":true,"token":"..."}`。初始登录名为 `admin`，改名后必须使用新名称；不接受仅密码登录。无需模拟浏览器，也不要用 `/api/account/login` 提取 body token，该接口只设置账号 Cookie。

管理请求使用 `Authorization: Bearer <原始token>`。无 Cookie 的运维 Bearer 路径不要求 `X-Tokengo-Panel`；Cookie 与 Authorization 同时出现会被拒绝。浏览器请求仍须遵守前文 CSRF 规则。数据库中存储的是摘要，不能把摘要当作 Bearer 使用。

任务结束后调用 `POST /api/auth/logout`，以 JSON `{"token":"原始token"}` 撤销该运维会话。**只带 Authorization 而提交空对象不会撤销会话**；返回 `ok:true` 也不能替代失效验证。root 改名、改密或安全重置会撤销相关新旧会话；token 过期或撤销后，管理请求返回401，需重新登录。不要通过直接编辑数据库修复凭据状态。

下面的 Python 3 示例只读取渠道数量，不打印密码、token 或渠道配置；仅创建并撤销本次运维会话。保存为 `panel-session-example.py` 后运行 `python3 panel-session-example.py`。使用本机 loopback HTTP 或可信 HTTPS 地址；脚本不跟随重定向、不使用系统代理、不保存 Cookie，TLS 证书校验保持开启。

```python
import getpass
import json
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import (
    HTTPRedirectHandler, ProxyHandler, Request, build_opener,
)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


base = input("实例地址 [http://127.0.0.1:3160]: ").strip().rstrip("/")
base = base or "http://127.0.0.1:3160"
url = urlsplit(base)
if (url.scheme not in ("http", "https") or not url.hostname
        or url.username is not None or url.password is not None
        or url.query or url.fragment or url.path not in ("", "/")
        or (url.scheme == "http"
            and url.hostname not in ("127.0.0.1", "localhost", "::1"))):
    raise SystemExit("请使用实例根地址；远程访问必须使用可信 HTTPS")
opener = build_opener(ProxyHandler({}), NoRedirect())


def request(method, path, body=None, token=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    data = None if body is None else json.dumps(body).encode()
    req = Request(base + path, data=data, headers=headers, method=method)
    try:
        with opener.open(req, timeout=15) as response:
            return response.status, json.load(response)
    except HTTPError as error:
        status = error.code
        error.close()
        return status, {}


username = input("当前 root 登录名: ").strip()
password = getpass.getpass("root 密码: ")
status, login = request("POST", "/api/auth/login",
                        {"username": username, "password": password})
del password
token = login.pop("token", None)
if status != 200 or not isinstance(token, str) or not token:
    raise SystemExit(f"登录失败，HTTP {status}")
try:
    status, channels = request("GET", "/api/channels", token=token)
    if status != 200 or not isinstance(channels.get("channels"), list):
        raise RuntimeError(f"读取渠道失败，HTTP {status}")
    print("读取成功，渠道数:", len(channels["channels"]))
finally:
    status, logout = request("POST", "/api/auth/logout", {"token": token})
    if status != 200 or logout.get("ok") is not True:
        raise RuntimeError("撤销未确认；请按 root 凭据仍有效处理")
status, _ = request("GET", "/api/channels", token=token)
del token
if status != 401:
    raise RuntimeError(f"预期撤销后401，实际HTTP {status}；请核查会话状态")
print("会话已撤销，再次访问返回401")
```

401应检查登录名、密码、token 到期/撤销及 Cookie 混用；403可能来自 CSRF 或权限边界；429表示登录节流或认证资源繁忙，应延迟重试，不能循环尝试密码。网络超时不代表登出已完成；未确认撤销时保留为待处理状态，必要时通过 root 安全操作撤销全部相关会话。进程强制退出也可能跳过清理，不要把 `finally` 视为崩溃后的撤销保证。

本文说明的是当前源码行为。示例的本地隔离空库验收不能替代指定发布镜像或目标部署环境的验证。

## 统一用户与浏览器自助 Key

| API | 请求/行为 | 成功输出 |
| --- | --- | --- |
| GET /api/managed-users | root或独立运维令牌管理；统一用户、关联凭证安全元数据、现有金额预算；root账号仅向root投影 | root返回`{users,root_accounts}`，运维令牌仅返回`{users}` |
| POST /api/managed-users | `{username,password,display_name,policy?,limit_micro?}`；同一事务创建普通user/凭证/可选金额预算/审计；不接受user_id或root身份 | `{user}` |
| PATCH /api/managed-users/{id} | `{display_name?,enabled?,policy?,password?,limit_micro?}`；保留原ID；更新金额上限不清零spent/reserved；用户停用同步其登录状态 | `{user}` |
| POST /api/managed-users/{id}/login-credential | `{username,password,display_name?}`；仅为尚无凭证的历史用户补登录，不重挂既有账号 | `{user}` |
| GET /api/account/keys | 从普通Cookie会话推导owner，返回本人Key/预算/安全策略/获准可见模型/32把上限 | `{keys,policy,budget,models,max_active_keys}` |
| POST /api/account/keys | `{name,policy?,expires_at?,limit_micro?}`；顶级Key归属当前user，不要求先有VKey；事务内检查活动Key数 | `{key,secret}`，secret仅本次返回 |
| PATCH /api/account/keys/{id} | `{name?,policy?,enabled?,expires_at?,limit_micro?,revoke?}`；只能收紧本人Key策略；允许暂停、撤销及恢复自己暂停的未到期Key，恢复须通过祖先有效性和活动Key数量检查；不能恢复管理员停用或已撤销的Key | `{key}` |
| DELETE /api/account/keys/{id} | 撤销本人Key；非本人404；已撤销不能复活 | `{key}` |

统一管理入口支持root会话和明确登记的独立运维令牌，保留现有root运维Bearer兼容；独立令牌不返回root账号投影，也不因此获得root安全或令牌签发权限。普通自助Key接口只接受浏览器会话，Cookie写操作沿用CSRF门，Cookie/Bearer混合拒绝。所有新响应加no-store。模型选项不泄露未授权来源配置或凭据；数据库新Key只存安全摘要，列表永不返回secret。

新建 API Key 成功后，前端切换到独立的明文保存界面并自动尝试复制完整 Key；页面提供“复制 Key”重试和只读文本框手动复制。普通 HTTP 或剪贴板权限限制导致复制失败时明确提示，并保留本次明文，直到关闭交付界面或离开页面。管理员代建同样适用。历史 Key 只有摘要与提示标识，无法恢复明文；未保存时须新建替代 Key，确认客户端使用新 Key 后再停用旧 Key。

自助策略必须不宽于实时用户策略，管理能力强制关闭；编辑既有Key还须不宽于其旧策略和父链。金额上限不能高于用户、既有Key或父链上限，不能清空有限上限来逃限；多个Key仍共享用户总额。Key到期可缩短，不能超出既有或父链期限。密码修改/重置撤登录会话，保留API Key与财务身份；用户整体停用阻断登录和新API请求。

历史多个登录凭证共享一个user的记录保留，不自动合并或改账本归属；统一界面显示历史状态。此类用户不能通过统一入口批量重置所有密码，需兼容维护。旧 `/api/users`、`/api/accounts` 与持VKey的 `/v1/keys` 保留兼容，不是普通用户创建/登录的主界面流程。

用户/Key策略改为点选控件，保留null继承、空集合拒绝全部和旧隐藏值；未知字段阻止有损保存。第一阶段未实现policy版本号/409乐观并发冲突，不能声称已有跨页面旧值覆盖保护；请求时实时权限再验证继续生效。

本文件是当前实现合同。定向服务端与Web检查已通过，浏览器闭环、完整最终CI和生产发布按最终提交分别验收，不能从代码或旧版本部署推定已上线。

## 当前授权选择与用户删除

新授权候选仅列可见模型；管理员使用可见模型目录，普通用户还须满足父级真实来源权限。已有隐藏或目录外显式授权放在“已有兼容授权”折叠区，只能撤销；保存其他资料不会丢弃原值，取消不会提交撤销。目录失败时不把全部来源模型作为备选。空集合仍表示禁止全部，与 null 继承不同。

管理员可删除普通用户：`DELETE /api/managed-users/{id}` 归档稳定用户身份，撤销其全部 Key，移除登录凭证/会话和私有策略，保留请求、用量、账本及删除审计。存在该用户运行请求、预占或未结算项时拒绝；根管理员不能从普通用户删除入口移除。普通用户可恢复自己暂停的 Key，但不能恢复管理员停用、撤销或祖先禁用的 Key。
