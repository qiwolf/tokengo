# W11补充：SF文档与账号目录漂移

read_at：2026-10-28（Asia/Shanghai）。这是资料补充，不是新增来源能力或真实调用通过声明；历史research JSON保持原时点状态。

|官方材料|精确读取时间（UTC）|所取得HTTP HTML SHA256|
|---|---|---|
|[Chat API](https://docs.siliconflow.cn/docs/api/chat-completions-post)|2026-10-27T16:12:30.521674Z|`ef916bd8876edfa48663d1b995505e0394370c1fe78c6667569013e1f3c22356`|
|[推理指南](https://docs.siliconflow.cn/docs/userguide/capabilities/reasoning)|2026-10-27T16:12:30.689548Z|`9fb4fad531cd7a5119e7aa78227e62427f4a8c218d31a0d3d771c93c06ec311b`|
|[官方Yan接入页](https://docs.siliconflow.cn/docs/usercases/use-siliconcloud-in-yan)|2026-10-27T16:12:30.873732Z|`2df63672df120a23b6b767a3f58b18d81ad795f45af61f210e915efea2007ad3`|

hash标识当次HTTP HTML，不标识永不变化的纯正文。整页HTML只留本地验收目录，不随仓库公开。

当前Chat参数文档仍将effort说明关联旧Pro来源，枚举high/max并说明兼容映射；页面内公开schema的消息role列user/assistant/system，未列developer。这是通用schema资料，不等于已经实测每个模型接受或拒绝的结论。推理指南明确区分可见答案与思考，并说明非Qwen模型可能继续超过thinking_budget，因此128不是通用硬上界。

root在build-test使用同一受控账号读取[SF模型目录](https://api.siliconflow.cn/v1/models)，HTTP200、98项，记录的catalog摘要SHA256为 `dcb803328505f08f7f42046955bbfa592ec6e68a2a8d93608804c63ee928d965`。此数量/摘要仅对应当次账号，不代表全平台模型清单、调用权限或能力。脱敏观测报告本身SHA256为 `e4ad89abfb13e027d3ec64836a69938fa000b5161aeeedeea547e0084f90be6e`；凭据未记录。

当次可见的GLM含非Pro `zai-org/GLM-5.2`、5.3与5.3Flash，Pro列表有5.1，未见旧文档的 `Pro/zai-org/GLM-5.2`。DeepSeek当前V4/V4.1和Kimi Code的精确名称见[来源矩阵](w11-source-matrix.json)。不存在于该账号目录不证明全平台下线；新非Pro型号也不能自动继承旧Pro合同。

新增目录行只标 `observed_catalog`，reasoning/cap/runtime及工具、SDK均未据此验证。此前DeepSeek/GLM/Kimi三family尚未完成8call验收，不能写成不能使用。按用户新增要求，文档或实测缺口只标“未充分验收”，不据此关闭原生或已可表示的转换；root继续推进真实小probe并按具体失败归因。授权、归属与具体不可表达请求的拒绝另行区分。Qwen的前导角色和关闭思考经验仅限已证明的精确SF型号；其他family可调用与是否采用该适配是两件事，不从Qwen自动推导规则；最终候选仍需fresh Qwen8call，并绑定实际源码与二进制。

ignored harness仅增加受控runner的精确model环境选择，默认及原developer/none请求不变。不能用新场景的较简单请求成功替换原客户端控制场景失败，不为获取绿灯修改客户端或扩大品牌规则。

## 后续精确角色对照（2026-10-28）

root随后在同一受控SF来源执行两轮各6次direct调用、未自动重试。第一轮脱敏报告SHA256 `204ee25baab6e1bc0e2d0ef1a172d313abd8643844d71b1bf22bb3d45bd9e697`；第二轮把唯一输出指令放在控制角色里、user仅发送Proceed，以排除user自己携带marker造成的假阳性，报告SHA256 `7e1f2b35cc2a6aac95fc591123689f80c2c77d18f3ac70c68d57d7176ef664bc`。

对 `deepseek-ai/DeepSeek-V4-Flash`、`zai-org/GLM-5.2`、`moonshotai/Kimi-K2.7-Code`：system对照均HTTP200并执行唯一控制指令；developer对照中DeepSeek/GLM虽HTTP200却未执行该指令，Kimi返回明确role错误。由此仅为这三个精确SF ID复用前导纯文本developer→单system的已说明语义折叠；不推广Pro/未来版本或其他family，不由角色实验授予none、effort、strict或预算保证。

真实网关的后续JSON/SSE每family两次验收使用独立ignored入口，不改原Qwen8call的developer+none场景。fixture不配置source context，复用原价格准入输出1024作为本地估算，报告明确不是供应方总输出上界；实际usage独立记录并只结算一次。这个入口尚待最终源码执行，direct角色对照不能冒充网关端到端已通过。
