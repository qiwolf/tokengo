> 新版渠道以唯一API URL和多Key池为主操作入口，当前合同见[渠道Key池与用户私有调用策略](provider-key-pools.md)。原连接ID保全继续适用，当前删除合同见文末；旧版“分组不共享凭据并发”的描述仅指池引入之前。

# 服务商渠道与连接

服务商渠道是显式配置的父层；下面的连接仍保留原channel ID。相同域名、模型名或供应商品牌不自动合并凭据，父层不宣称共享并发、价格或账号配额。OAuth绑定、VKey allowed_channels、fallback目标和协议会话继续引用原连接ID。

迁移33新增provider_channels、provider_connections；每连接最多一个父层。未分组连接仍可见且照旧路由。channel_models.enabled默认true；停用不删除upstream/vision/drop_fields，重新启用恢复原配置。来源有效条件为父层启用（无父层则true）、连接启用、mapping启用、逻辑模型全局启用及原权限/能力/路由限制全部满足。

本页业务管理接口支持root会话及明确登记的独立运维令牌，具体方法与路径以 `/api/ops/capabilities` 为准，接入方式见[运维令牌](operations-tokens.md)。Cookie写入继承X-Tokengo-Panel:1及同源校验；旧无Cookie root运维Bearer保留兼容，API admin VKey不能管理这些接口。历史上游Key明文查看仍限root；独立运维令牌不能读取历史密钥或绑定OAuth账号，响应另执行机器脱敏投影。错误沿用error.message/type；无凭据值出现在新增DTO。

| 接口 | 请求 | 返回 |
| --- | --- | --- |
| GET /api/providers | 无 | `{providers:[Provider],ungrouped_connection_ids:[id]}` |
| POST /api/providers | `{name,enabled?:true}` | `{provider:Provider}` |
| PUT /api/providers/{id} | `{name,enabled}` | `{provider:Provider}` |
| DELETE /api/providers/{id} | 无 | 确认删除所属可变配置；存在运行请求或未结算项时拒绝 |
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

渠道首页以供应方为主体，模型管理、渠道编辑、上游 Key 管理及“诊断与分组”按任务打开。Key 编辑与模型选择为独立操作；模型卡片和操作行紧凑显示，状态与启停按钮分开。原生推理、协议字段及模型路由通过页面控件配置，不要求手写 JSON。

删除需明确确认，后端在同一个事务中执行以下规则：

- 删除渠道会清理其 Key、接入模型映射、来源白名单、失去全部来源的显式模型授权及相关路由引用；仍由其他来源声明的模型不会因此失去全部授权。最后一项显式授权移除后保留空集合，不能变成允许全部。
- 私有虚拟路由移除相应目标并重排顺序与时段引用；没有目标或出现空时段的策略停用，避免静默改走默认链。
- 删除独立接入时，如同一凭据仍有其他模型部署，保留其有效密钥承载；归属异常或损坏配置导致无法安全处理时整体回滚。
- 已有请求、尝试、财务账本及版本化价格历史保留，审计记录保存删除动作及来源身份；不会删除历史来绕过依赖。
- 存在进行中的请求、未结算账本或活跃恢复探测租约时拒绝删除，应待其完成后重试。停用适用于暂时不使用而希望保留完整配置的渠道。

用户删除另采用归档：撤销 Key、移除登录与私有路由，保留用户稳定身份及账务记录。以上是已实现的生命周期规则，不代表所有历史阶段都采用相同删除策略；发布与部署状态见[验收状态](acceptance-status.md)。

## 本地向量节点等权轮询

模型路由可配置 `local_strategy: "round_robin"`，在本地Embeddings/Rerank获授权来源间按请求轮转起点，并沿用故障切换；`ordered`或省略继承已有Key策略。它与在线Key按用户粘性不同，不影响OAuth/native来源固定。游标按调用入口、owner、provider及目标模型隔离，进程重启后重置；不是分布式加权或等GPU利用率承诺。来源身份变化后需刷新兼容证据。详见[本地节点合同](maintenance/local-vector-balancing.md)。
