# 服务商渠道与连接

服务商渠道是显式配置的父层；下面的连接仍保留原channel ID。相同域名、模型名或供应商品牌不自动合并凭据，父层不宣称共享并发、价格或账号配额。OAuth绑定、VKey allowed_channels、fallback目标和协议会话继续引用原连接ID。

迁移33新增provider_channels、provider_connections；每连接最多一个父层。未分组连接仍可见且照旧路由。channel_models.enabled默认true；停用不删除upstream/vision/drop_fields，重新启用恢复原配置。来源有效条件为父层启用（无父层则true）、连接启用、mapping启用、逻辑模型全局启用及原权限/能力/路由限制全部满足。

所有新管理接口限root PanelAuth，Cookie写入继承X-Tokengo-Panel:1及同源校验。旧无Cookie运维Bearer兼容，API admin VKey不能管理这些接口。错误沿用error.message/type；无凭据值出现在新增DTO。

| 接口 | 请求 | 返回 |
| --- | --- | --- |
| GET /api/providers | 无 | `{providers:[Provider],ungrouped_connection_ids:[id]}` |
| POST /api/providers | `{name,enabled?:true}` | `{provider:Provider}` |
| PUT /api/providers/{id} | `{name,enabled}` | `{provider:Provider}` |
| PUT /api/channels/{id}/provider | `{provider_id:id或null}` | `{ok:true}` |
| PUT /api/providers/{id}/models/enabled | `{model,enabled,connection_ids:[id]}` | `{ok:true,updated_connection_ids:[id]}` |
| PUT /api/channels/{id}/models/enabled | `{model,enabled}` | `{ok:true}` |
| PUT /api/model-visibility?model=URL编码名字 | `{visible}` | `{ok:true,model,visible}` |
| PUT /api/channels/{id}/models/reasoning | `{model,manual_levels:null或[string],default_effort:null或string}` | `{reasoning:ReasoningCapabilities}` |

Provider为`{id,name,enabled,connection_ids,models:[{name,visible,global_enabled,declared_connection_ids,enabled_connection_ids,routable_connection_ids}]}`。enabled_connection_ids只描述mapping本身开关；routable_connection_ids同时考虑父层/连接/全局模型/路由限制，不承诺下一次请求供应商一定成功。批量开关要求显式1..256个不同正ID，全部连接必须仍属于该父层且声明该logical；一次验证后事务写入，任一错误不部分修改。无父层批量默认scope、无隐式全量扩张。

旧GET /api/channels新增provider_id/provider_name/provider_enabled，model项新增enabled与reasoning。旧连接save的model项可传enabled；未传时同名既有mapping保留状态，新mapping默认启用。显式删mapping仍是高级删除操作，不等价普通停用。

GET /api/model-routes保留所有原字段，新增visible及sources：`{connection_id,connection_name,provider_id,provider_name,provider_enabled,connection_enabled,model_enabled,reasoning}`。declared_by保留所有声明映射，routed是最终全局可用事实；UI可以展示停用项以恢复。兼容旧alias可显式设visible=false，从/v1/models和auto排除，但直接请求继续可用；名称本身不被用于推断档位或隐藏规则。

ReasoningCapabilities字段为`{status,levels,source,reason,default_effort,configured_default,manual_levels}`。按每个连接+logical读取，不把多个来源档位求并集冒充所有来源都支持。目录真实声明或明确人工声明才可生成levels；单独boolean reasoning不推断low/high。默认仅在客户端未传原生参数时补充，配置null表示沿供应商默认；Chat reasoning_effort与Responses reasoning.effort由受测转换器处理。不通过模型名后缀选择推理档位，无法表示的协议语义仍拒绝。旧OAuth连接级low/medium/high绑定枚举保留兼容；新档位默认通过mapping配置，不重建OAuth账号或表。

本轮不迁移或重写历史账本/价格/密钥/连接ID、不做真实供应商推理验收。新增能力的实际回归与部署状态另见交付证据。
