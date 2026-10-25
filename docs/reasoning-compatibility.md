# 推理关闭与档位兼容

`none`表示明确关闭推理，与省略参数、跟随服务商默认不同。TokenGo保留客户端显式选择，不把所有模型默认改成none，也不按模型名中的品牌或后缀推断能力。

## 目录与分发规则

目录支持显式档位字段，并读取DeepSeek原生的`effort.supported_levels`、`effort.default_level`。格式错误、矛盾声明及不在列表内的默认值保持未知，不采用部分结果。原始目录缓存保持原样。

在当前来源身份仍有效且目录明确支持推理时，以下规则随源码、镜像和二进制分发：官方OpenAI API/OAuth的精确`gpt-6-luna`补齐`none`；官方DeepSeek端点补齐其目录未列出的关闭选项。面板以“上游声明 + 内置兼容”标明来源。管理员手工列表优先，可排除none；原服务商默认值和管理员默认值均保留。既有数据库缓存在读取时应用规则，新建连接读取目录后即可使用，不注入生产渠道、账号、地址、私钥或价格。

第三方转发、自建兼容服务和未知供应商不套用官方来源规则。它们明确声明的none仍按声明提供，也可通过每模型推理参数人工确认；其原生字段语义由实际服务商合同决定。

## 协议语义

|来源|Responses|Chat|Messages|
|---|---|---|---|
|官方OpenAI Luna|`reasoning.effort: none`|`reasoning_effort: none`，按来源协议转换|不据此宣称原生Messages支持|
|官方DeepSeek|`reasoning.effort: none`|转换成`thinking: {type: disabled}`，不发送`reasoning_effort: none`|转换成`thinking: {type: disabled}`，不发送`output_config.effort: none`|

DeepSeek的Chat/Messages中，客户端显式thinking开关优先于管理员默认；显式none与thinking enabled冲突时拒绝，不静默覆盖。非none的effort和其他原生字段保持原语义。DeepSeek已文档化的minimal/medium/xhigh/ultra客户端别名，在目录含相应规范档位时保留原值透传，不因新增目录识别而收窄；管理员手工列表仍严格优先。Responses/Messages跨协议转换时，只要选中连接的有效模型能力明确含有`none`（包括内置兼容补齐或第三方明确声明），就保留该关闭意图，恢复目标协议的原生effort字段；随后再按DeepSeek原生合同适配thinking开关。已知能力不含none、管理员手工排除none或明确不支持推理时，出站前拒绝，不得删除none后注入high等默认值。未知能力仍保留旧CLI转换归一化合同；这不代表已关闭上游默认推理，不把删除字段视为关闭。

这些是确定的元数据/请求转换规则；内置规则不代表每个账号或未来模型已完成真实推理验证。Luna的OAuth none曾有独立现场普通/流式严格schema实测，DeepSeek依据官方原生合同及本版回归测试；不能扩大为所有供应商实测。

## 升级与回退

无需复制现有用户数据库，无schema变更。升级保留已有manual_levels/default_effort。对旧版本回退时，内置补齐和原生关闭适配也随程序回退，不能仅保留none配置就宣称旧二进制正确实现关闭。保留数据库、主密钥和账务历史，不用旧整库覆盖新账单。

官方依据：[OpenAI Luna](https://developers.openai.com/api/docs/models/gpt-6-luna)、[DeepSeek模型目录](https://api-docs.deepseek.com/api/list-models/)、[DeepSeek推理开关](https://api-docs.deepseek.com/guides/thinking_mode/)。
