# 透传服务与用户授权

HTTP 透传服务按路径前缀调用，独立于 LLM 模型协议；服务管理复用 `/api/passthrough-routes` 的新增、修改、删除和检查接口。路由 ID 不重用，删除保留历史账目。

用户和 Key 的权限 JSON 增加 `allowed_services`，保存于既有 `policy_json`，不迁移数据库表。

| 身份 | null 或省略 | ID 数组 | 空数组 |
| --- | --- | --- | --- |
| 用户 | 保持旧逐 Key 白名单，不新增用户全 Key 权限 | 显式授予该用户的所有有效 Key | 全部拒绝，包括旧白名单 |
| Key / 祖先 Key | 继承，不额外缩小 | 在用户与祖先范围内缩小 | 全部拒绝 |

管理员显式给用户选择服务，会使其新旧 Key 继承用户授权。首次读取旧用户时不得自动把旧逐 Key 白名单转换为用户全 Key 授权。普通用户不能通过自助创建或编辑 Key 为旧用户身份新增服务授权；对 Key 已有的缩小限制也不能通过改回 null 擦除。

路径白名单、渠道范围、付费开关、预算、RPM、并发、停用/撤销/过期及祖先限制保持生效；TPM 限制仍不适用于无法核算 Token 的透传调用。队列释放后会重新读取服务修订号和实时权限，撤权后不再发送上游请求。

普通用户 `GET /api/account/services` 返回 `{ services, policy }`，`GET /api/account/keys` 同时附带 `services`。每项只含 `id,name,prefix,enabled,channel_id,price_micro_usd_per_request`。目录仅展示用户显式获准或其现有有效 Key 白名单获准的服务，不返回上游地址、Header 或其他 Key ID。目录展示不代替实时调用鉴权。

管理员新增授权校验服务 ID 存在；服务删除后允许保留和移除历史引用，不能新增不存在的 ID。
