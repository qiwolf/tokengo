# 部署者智能体的运维令牌

自 `v0.34.0-qiwolf.5` 提供，数据库 schema 38。部署时以 `/api/ops/capabilities` 的实际返回确认可用动作。

运维令牌用于部署者信任的智能体通过 HTTP 管理本站。无需把 root 密码、浏览器 Cookie 或兼容登录会话交给智能体。所有令牌只有一档固定权限：`site_operations`（站点运维），没有 scope、多档角色或永久有效选项。

## 创建、使用与撤销

部署者以 root 浏览器账号进入“设置 → 运维令牌”，输入名称和有效期，并确认当前密码。完整令牌仅创建成功时展示一次；关闭后无法重新读取，丢失时应创建新令牌并撤销旧令牌。令牌格式为 `tgo_ops_<public_id>.<secret>`，数据库仅保存摘要。

名称为 1–64 个字符；有效期默认 90 天，可选 1–365 天；同站点最多 20 把未到期且未撤销的令牌。最近使用时间不延长有效期。按用途分别签发，便于审计与单独撤销。令牌应放入智能体秘密存储，不放进聊天、代码、URL、日志或知识库。

每个请求只发送一个 `Authorization: Bearer <令牌>`，不发送 Cookie，不在 query/body 传令牌。Cookie 混用、重复 Authorization 都会失败。生产地址使用 HTTPS；本机 loopback 验证允许 HTTP。不要盲信任客户端可控的代理协议头。

推荐接入流程是先请求 `/api/ops/me`，核对预期实例和令牌名称；再读取 `/api/ops/capabilities` 的实际动作清单；最后调用对应业务 API。不要仅凭产品版本猜测接口权限。

普通 root 改名或改密不撤销运维令牌；疑似泄露时应立即撤销相关令牌，或由部署者执行安全重置，同时撤销全部浏览器/兼容会话和运维令牌。撤销和到期立即阻止后续授权，服务重启也不会复活令牌；已经产生的外部探测费用及已经返回的数据无法撤回。

轮换使用“创建 → 自检 → 更新智能体秘密存储 → 撤销旧令牌”。不提供刷新、延期或自动续期接口。

## 适用范围与信任边界

这是高权限站点凭据，可配置流量目的地、替换供应商凭据、签发业务 Key、管理普通用户和费用策略，适合部署者信任的智能体。禁止历史凭据 reveal 并不构成对恶意智能体的隔离：它可间接签发推理 Key。

开放的日常运维包括供应方、渠道、模型目录和映射、路由与探测、普通用户及其业务凭据、定价与预算，以及脱敏诊断。探测可能实际调用供应商并产生费用。保留现有对象引用、归属、预算、不可复活和账本约束。

root 身份安全、面板会话、令牌管理、OAuth 授权与绑定、历史密钥明文、原件导出、立即 prune、恢复和整体重置不开放给机器。`/api/settings` 只读允许字段；`PUT /api/settings` 整体拒绝机器调用，不提供任意 settings 写入；路由配置通过 `/api/routing/config`。`/v1/*` 不接受运维令牌作为推理或用户自助凭据，有效运维令牌访问返回 403。

诊断只输出允许的结构化元数据，不输出原始提示词、响应正文、原始上游错误体、自由日志文本、Cookie、认证头或凭据快照。`GET /api/logs/live` 仅返回日志条数、当前级别和恢复时间等元数据，不返回原始日志行或自由文本。如果上游把当前渠道的认证密钥或自定义认证头值回显进模型 ID、归属方等字符串字段，服务端会拒绝保存该目录；读取历史数据时发现同类回显，也会返回不包含原值的通用 502。机器写入的审计身份为独立运维令牌，不冒充 root；审计通过 `/api/ops/audit` 读取。本站不会提供 shell、宿主重启、容器升级、任务编排或发布能力。

## API 合同

未发布的[通用 HTTP 透传](passthrough.md)管理及诊断接口 `/api/passthrough-routes*` 仅允许 root，未加入运维令牌动作清单；不能用普通渠道或价格接口修改其内部来源。运维令牌也不能作为透传业务 VKey。

下列令牌管理接口仅允许 root 浏览器会话，同源写操作需要 `X-Tokengo-Panel: 1`。旧兼容 Bearer、管理员 VKey 和运维令牌均不能管理运维令牌。

| 方法 | 路径 | 行为 |
| --- | --- | --- |
| GET | `/api/ops-tokens` | 安全元数据分页：`{tokens,next_before_id}`，无 secret 或摘要 |
| POST | `/api/ops-tokens` | `{name,password,expires_in_days?}` → `{token,secret}`；`secret` 是一次展示的完整 Bearer |
| DELETE | `/api/ops-tokens/{id}` | 幂等撤销单把令牌，不需再次输入密码 |
| POST | `/api/ops-tokens/revoke-all` | `{password}`，撤销事务前已有有效令牌 |
| POST | `/api/account/security-reset` | `{password}`，安全重置并撤销所有会话及运维令牌 |

`token` 元数据包括 `id,public_id,name,created_by_account_id,created_at_ms,expires_at_ms,revoked_at_ms,last_used_at_ms,status`；时间为 UTC Unix 毫秒，状态为 `active/expired/revoked`。

机器专用入口：

| 方法 | 路径 | 返回 |
| --- | --- | --- |
| GET | `/api/ops/me` | `{instance_id,token,permission:"site_operations",contract_version:1}` |
| GET | `/api/ops/capabilities` | `{contract_version:1,actions:[{method,path}],documentation}` |
| GET | `/api/ops/audit` | `{events,next_before_id}`；事件包括机器 actor、动作、目标、请求关联与成功状态 |

实际接入的业务 method/path 以 `/api/ops/capabilities` 返回为准，该清单与服务端授权注册表同源；新增路由默认拒绝，不能推断所有 `/api/*` 均可用。具体动作见下节。

身份无效、过期或撤销统一返回 `401 invalid_operations_token`，不暴露失效细节。有效机器身份访问禁止动作返回 `403 operations_action_forbidden`；真正不存在的路径为 404。浏览器会话有效但确认密码错误返回 `403 step_up_failed`，保留当前登录；字段错误 400，业务冲突沿用现有错误语义，资源限制 429，存储故障 5xx；存储失败时不继续授权。令牌管理和机器响应均为 `Cache-Control: no-store`。

## 可执行 Python 只读接入示例

使用 Python 3 标准库，无需安装依赖。完整令牌从隐藏输入读取；脚本不需要 root 密码，不打印令牌，不保存令牌，不跟随重定向，避免把 Authorization 带到其他地址。示例仅自检及读取渠道，不修改或删除现有对象。将代码保存为 `ops_check.py` 后运行 `python3 ops_check.py`。

```python
import getpass
import ipaddress
import json
import sys
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    base = input("实例地址（例如 https://gateway.example.com）：").strip().rstrip("/")
    parsed = urllib.parse.urlsplit(base)
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("实例地址不能包含用户凭据、query 或 fragment")
    loopback = parsed.hostname == "localhost"
    try:
        loopback = loopback or ipaddress.ip_address(parsed.hostname or "").is_loopback
    except ValueError:
        pass
    if not parsed.hostname or parsed.path not in ("", "/"):
        raise ValueError("请填写实例根地址")
    if parsed.scheme != "https" and not (parsed.scheme == "http" and loopback):
        raise ValueError("远程实例必须使用 HTTPS；HTTP 仅允许 loopback")
    expected = input("预期 instance_id（首次接入可留空，之后建议填写）：").strip()
    token = getpass.getpass("运维令牌（隐藏输入）：").strip()
    if not token.startswith("tgo_ops_") or len(token) > 256:
        raise ValueError("令牌格式不正确")
    opener = urllib.request.build_opener(NoRedirect())

    def read(path):
        req = urllib.request.Request(base + path, headers={
            "Authorization": "Bearer " + token,
            "Accept": "application/json",
        }, method="GET")
        try:
            with opener.open(req, timeout=20) as response:
                if response.headers.get_content_type() != "application/json":
                    raise ValueError("响应不是 JSON，请核对实例地址")
                raw = response.read(2 * 1024 * 1024 + 1)
                if len(raw) > 2 * 1024 * 1024:
                    raise ValueError("响应超出脚本读取上限")
                return json.loads(raw)
        except urllib.error.HTTPError as exc:
            # 不打印服务器响应体、请求头或 URL query。
            raise RuntimeError("运维请求失败，HTTP " + str(exc.code)) from None
        except urllib.error.URLError:
            raise RuntimeError("连接失败，请核对网络、TLS 和实例地址") from None

    me = read("/api/ops/me")
    if me.get("contract_version") != 1 or me.get("permission") != "site_operations":
        raise ValueError("实例合同或权限与预期不符")
    instance_id = me["instance_id"]
    if expected and expected != instance_id:
        raise ValueError("instance_id 不符，已停止后续访问")
    print("实例：", instance_id)
    print("令牌：", me["token"]["name"])
    capabilities = read("/api/ops/capabilities")
    if capabilities.get("contract_version") != 1:
        raise ValueError("动作清单合同版本不符")
    if not any(a.get("method") == "GET" and a.get("path") == "/api/channels"
               for a in capabilities["actions"]):
        raise ValueError("实例未开放渠道读取")
    channels = read("/api/channels")
    # 只打印数量，避免把渠道元数据写入通用终端日志。
    rows = channels if isinstance(channels, list) else channels.get("channels", [])
    print("渠道数量：", len(rows))
    print("只读自检完成。")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, RuntimeError) as exc:
        print(str(exc), file=sys.stderr)
        sys.exit(1)
```

令牌撤销后重复运行应获得 HTTP 401。浏览器界面完成撤销；不要把 root 密码补进此脚本。写入、撤销、过期和审计验证使用隔离临时数据库及受控测试对象，见 `crates/server/tests/operations_tokens.rs`。测试不会证明真实供应商探测、目标部署或生产智能体接入已验收。

## 已登记业务动作

以下清单由服务端动作注册表整理；路径中的 `{id}` 等为模板参数。部署时可再次调用 capabilities 核对实际运行版本。未列明动作不应尝试绕过授权。

<!-- OPERATIONS_ACTIONS:START -->
| 方法 | 路径 |
| --- | --- |
| GET | `/livez` |
| GET | `/readyz` |
| GET | `/api/ping` |
| GET | `/api/ops/me` |
| GET | `/api/ops/capabilities` |
| GET | `/api/ops/audit` |
| GET | `/api/channels` |
| POST | `/api/channels` |
| PUT | `/api/channels/{id}` |
| DELETE | `/api/channels/{id}` |
| PUT | `/api/channels/{id}/enabled` |
| POST | `/api/channels/{id}/probe` |
| POST | `/api/channels/{id}/models` |
| GET | `/api/channels/{id}/catalog` |
| GET | `/api/channels/{id}/registry` |
| PUT | `/api/channels/{id}/registry` |
| GET | `/api/channels/{id}/context` |
| PUT | `/api/channels/{id}/context` |
| POST | `/api/channels/{id}/vision-test` |
| POST | `/api/channels/{id}/move` |
| GET | `/api/providers` |
| POST | `/api/providers` |
| PUT | `/api/providers/{id}` |
| DELETE | `/api/providers/{id}` |
| GET | `/api/providers/{id}/credentials` |
| POST | `/api/providers/{id}/credentials` |
| PATCH | `/api/providers/{id}/credentials/{credential}` |
| DELETE | `/api/providers/{id}/credentials/{credential}` |
| POST | `/api/providers/{id}/models/discover` |
| PUT | `/api/providers/{id}/models/enabled` |
| PUT | `/api/channels/{id}/provider` |
| PUT | `/api/channels/{id}/models/enabled` |
| PUT | `/api/channels/{id}/models/reasoning` |
| PUT | `/api/model-visibility` |
| GET | `/api/channels/{id}/tools` |
| PUT | `/api/channels/{id}/tools` |
| GET | `/api/model-kinds` |
| GET | `/api/auto-policy` |
| PUT | `/api/auto-policy` |
| POST | `/api/model-order/move` |
| PUT | `/api/model-enabled` |
| POST | `/api/channel-faults/retry` |
| GET | `/api/model-cooldowns` |
| POST | `/api/model-cooldown/reset` |
| PUT | `/api/models/{name}/kind` |
| GET | `/api/model-routes` |
| GET | `/api/routing` |
| PUT | `/api/routing/config` |
| POST | `/api/routing/{model}/reprobe` |
| GET | `/api/model-routing` |
| PUT | `/api/model-routing` |
| DELETE | `/api/model-routing` |
| GET | `/api/stats` |
| GET | `/api/logs` |
| GET | `/api/requests/{id}` |
| GET | `/api/logs/live` |
| GET | `/api/logs/level` |
| PUT | `/api/logs/level` |
| GET | `/api/settings` |
| GET | `/api/maintenance` |
| POST | `/api/maintenance` |
| GET | `/api/gateway-keys` |
| POST | `/api/gateway-keys` |
| DELETE | `/api/gateway-keys/{id}` |
| GET | `/api/users` |
| POST | `/api/users` |
| GET | `/api/users/{id}` |
| PATCH | `/api/users/{id}` |
| PUT | `/api/users/{id}` |
| DELETE | `/api/users/{id}` |
| GET | `/api/users/{id}/keys` |
| POST | `/api/users/{id}/keys` |
| PATCH | `/api/vkeys/{id}` |
| GET | `/api/managed-users` |
| POST | `/api/managed-users` |
| PATCH | `/api/managed-users/{id}` |
| DELETE | `/api/managed-users/{id}` |
| POST | `/api/managed-users/{id}/login-credential` |
| GET | `/api/pricing` |
| POST | `/api/pricing` |
| GET | `/api/request-pricing` |
| POST | `/api/request-pricing` |
| GET | `/api/budgets` |
| PUT | `/api/budgets` |
| GET | `/api/usage` |
| GET | `/api/custom-probes` |
| POST | `/api/custom-probes` |
| PUT | `/api/custom-probes/{id}` |
| DELETE | `/api/custom-probes/{id}` |
| POST | `/api/custom-probes/{id}/run` |
<!-- OPERATIONS_ACTIONS:END -->

## 兼容入口

原 `/api/auth/login` 兼容 root Bearer 会话继续保留，其权限、有效期和改密撤销语义不转换为运维令牌。新自动化接入优先使用本文流程；兼容说明见 [账号与登录](account-login.md)。本次不提供 Issue #4 提出的细粒度 scope，也不承诺旧兼容接口长期稳定。
