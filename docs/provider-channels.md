> 新版渠道以唯一API URL和多Key池为主操作入口，当前合同见[渠道Key池与用户私有调用策略](provider-key-pools.md)。下文的原连接ID保全与删除依赖规则继续适用；旧版“分组不共享凭据并发”的描述仅指池引入之前。

# 服务商渠道与连接

服务商渠道是显式配置的父层；下面的连接仍保留原channel ID。相同域名、模型名或供应商品牌不自动合并凭据，父层不宣称共享并发、价格或账号配额。OAuth绑定、VKey allowed_channels、fallback目标和协议会话继续引用原连接ID。

迁移33新增provider_channels、provider_connections；每连接最多一个父层。未分组连接仍可见且照旧路由。channel_models.enabled默认true；停用不删除upstream/vision/drop_fields，重新启用恢复原配置。来源有效条件为父层启用（无父层则true）、连接启用、mapping启用、逻辑模型全局启用及原权限/能力/路由限制全部满足。

所有新管理接口限root PanelAuth，Cookie写入继承X-Tokengo-Panel:1及同源校验。旧无Cookie运维Bearer兼容，API admin VKey不能管理这些接口。错误沿用error.message/type；无凭据值出现在新增DTO。

| 接口 | 请求 | 返回 |
| --- | --- | --- |
| GET /api/providers | 无 | `{providers:[Provider],ungrouped_connection_ids:[id]}` |
| POST /api/providers | `{name,enabled?:true}` | `{provider:Provider}` |
| PUT /api/providers/{id} | `{name,enabled}` | `{provider:Provider}` |
| DELETE /api/providers/{id} | 无 | 空服务商 `{deleted:true}`；有关联连接返回409 `provider_not_empty` |
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


## 渠道界面与删除边界

渠道首页以服务商为主体，每张卡直接提供“管理模型”“编辑服务商”“删除服务商”。编辑服务商可修改名称和总启停，不需要先找高级设置。模型列表的开关控制此服务商内该逻辑模型的显式连接集合；停用不删除映射、原生档位或其他模型配置。

接口地址、凭据、协议和各连接ID在“连接设置”详情内展开，不把每条同名技术连接铺在渠道首页。未分组与旧连接默认收起，仍可按原ID编辑、停用或移动到明确服务商；页面整理不会自动合并或删除来源。编辑连接直接打开基本信息，模型映射仍可在同一编辑器切换管理。

删除均需明确确认，并由后端再次检查，不能仅靠前端隐藏按钮：

- 只有空服务商可以删除。存在关联连接时返回409 `provider_not_empty`；应先移动连接，或删除符合条件的无依赖连接。删除服务商不会级联删除连接。
- 连接 `DELETE /api/channels/{id}` 仅允许没有业务依赖的来源。若有 OAuth 绑定、用户或API Key的来源白名单、模型回退配置、原生会话来源固定，或请求/尝试/财务历史引用，返回409 `connection_in_use`，错误说明具体阻止类别。无法解析的授权或会话记录也保守拒绝，不把“无法确认”当作无依赖。
- 有历史引用的连接本轮不提供物理删除或归档迁移，请使用现有停用开关：停止参与新路由，保留来源ID、模型映射、授权、OAuth绑定和完整历史。不会自动解除引用、改写ACL或删除旧账本以满足删除条件。
- 无依赖连接可在确认后真正删除，移除该连接的地址、凭据与模型等从属配置；不存在返回404。删除条件和删除在同一个事务中检查执行。

本轮没有新增schema或归档系统；上述保护不宣称彻底静默正在执行的所有后台任务。编辑、停用、移动和删除的实际发布状态以[验收状态](acceptance-status.md)为准。
