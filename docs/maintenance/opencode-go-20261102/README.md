# OpenCode Go 模型目录与请求头修复（2026-11-02）

已通过线上管理界面修复既有 OpenCode Go 渠道。原地址为 `https://opencode.ai/inference/openai/v1`，目录读取返回 HTTP404；原接入没有自定义请求头。

## 配置结果

- API 地址改为 `https://opencode.ai/zen/go/v1`。此兼容入口本次实测目录成功；线上现有 Key 的选择模型面板恢复显示28个模型。
- 新增 `User-Agent: TokenGo/0.34.0`，标识实际网关。
- 新增 `x-opencode-session: {{session_id}}`，使用既有会话模板，不能用每次变化的 `{{uuid}}` 或全渠道固定值代替。

本次没有修改应用代码、密钥、模型映射、全局默认路由或其他供应商。没有开启额外付费用量。原接入未注册模型，本次仅恢复目录能力，不代替用户选择模型。

新版 OpenAI Go 推理前缀为 `https://opencode.ai/inference/go/openai/v1`，模型目录则为 `https://opencode.ai/inference/go/v1/models`。当前 TokenGo 直接在 API 地址后拼接 `/models`，故仅补上 `/go` 仍不足以使用新版前缀自动发现。本次使用实际仍可达的兼容入口，不声称实现了新版目录独立配置。

## 请求头与会话边界

[官方 Go 要求](https://opencode.ai/v2/docs/console/go/)包括编码代理用途、真实独立的客户端标识，以及每段对话稳定的 `x-opencode-session`。Bearer API Key 沿用既有认证流程；目录请求不生成会话 ID，空模板头会省略。

现有源码将显式会话解析、隔离后传入各协议的上游调用。客户端宜发送稳定的 `x-opencode-session` 或 `x-session-id`；Responses 也支持 `client_metadata.session_id`。同一对话保持相同，不同对话使用不同 ID，`prompt_cache_key` 不替代会话身份。

没有显式 ID 时，Chat/Messages 可根据首条 user 正文推导；首条消息被裁剪或不同对话使用相同开场时，不能保证区分。Responses 的 `input` 不适用该首条消息推导，正文增长会改变退化哈希，因此应显式传入会话 ID。本次没有扩大匿名会话推导设计。

## 真实调用的限制

使用用户提供的凭据、合成编程输入、独立客户端标识及稳定会话头，访问兼容入口的 DeepSeek V4.1 Flash、GLM-5.2，以及新版入口的 DeepSeek V4.1 Flash，均收到 HTTP429 `GoUsageLimitError` / `Go usage limit exceeded`；一次原始错误明确标注 `limitName: monthly`。

因此模型目录已恢复，但真实生成成功、流式输出、工具调用和模型参数兼容性没有通过本轮验收；月额度耗尽阻塞这些检查。HTTP429 不应解释成目录404、密钥无效或会话模板渲染失败。不同模型的额度范围和恢复时间应以供应商控制台为准。

## 验证与回退

线上原有 Key 的模型选择器已从404变为28个可选模型；接入设置显示2条自定义头。配置改动经原管理接口保存，服务无需重启。

回退仅针对本次渠道：将地址恢复为上述原地址，并移除新增的两个头；回退会恢复原目录错误。执行前应核对是否已有后续配置变更。

本地脱敏原始证据、截图及远端测试收据在工作区 `review/opencode-go-models-20261102/`；凭据不进入文档、Git或共享知识。具体测试结果见下节收据。

### 本轮测试收据

在 build-test 独立副本 `/srv/build/tokengo-opencode-go-20261102/source`，同步 Mac 源码版本 `84ad4a8386f36dd3260afac9ee5d590515578079` 并核对文件哈希后执行：

- provider_catalog：10项通过；sessions：6项通过。
- proxy 的 session 筛选：2项通过。
- server lib 的 session 筛选：41项通过；shared lib 的 session 筛选：2项通过。

共61项通过，没有重跑无关全套测试。首次 lib 检查因构建副本遗漏编译期引用的版本资料 sources.json 而未编译，补同步该原文件后受影响检查通过；首次失败日志保留。此为现有行为回归，不是新增代码测试或全协议真实上游验收。日志及 source-manifest、命令、退出码已全部取回 Mac 工作区的 `review/opencode-go-models-20261102/build-test/`；没有生成发行产物。
