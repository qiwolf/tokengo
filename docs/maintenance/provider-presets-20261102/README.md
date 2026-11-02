# 常用服务商图标与最新API地址预设（2026-11-02）

新建渠道新增 OpenCode、CommandGo、OpenDesign 品牌图标。OpenCode 分成 Go 与按量两个选项，避免误用套餐路径。选择后自动填入地址、默认名称及API认证，清除旧OAuth选择；自定义渠道名称和手工编辑地址的能力保留。

| 选项 | 默认API URL | 官方依据 |
| --- | --- | --- |
| OpenCode Go | `https://opencode.ai/inference/go/openai/v1` | [入口与会话](https://opencode.ai/v2/docs/console/harnesses) |
| OpenCode 按量 | `https://opencode.ai/inference/openai/v1` | 同上 |
| CommandGo | `https://api.commandcode.ai/provider/v1` | [Command Code Provider API](https://commandcode.ai/docs/provider) |
| OpenDesign | `https://amr-link.open-design.ai/v1` | [Cloud API连接设置](https://open-design.ai/zh/help/cloud-api/#api-keys)中的官方截图 |

OpenDesign地址已核对官网API keys的Chat Completions连接设置，不使用控制面URL，也不以CLI runtime配置或用户issue作为唯一依据。CommandGo对应Command Code品牌，应使用具有API权限的套餐；预设不等于账号授权、额度或每个模型协议的验收。

## 功能与边界

服务商预设与模型家族分开。三家默认使用“其他 / 多家族”网络家族，不向出口配置传入无效的新家族ID；不根据品牌自动授予模型能力。随后改选模型家族会退出服务商预设，并仅在当前地址仍是已知默认值时自动切换地址；自填地址保留。

OpenCode新版协议入口的目录请求精确转到同源共享目录 `/inference/go/v1/models` 或 `/inference/v1/models`，推理地址不变。仅识别官方HTTPS主机、标准端口、无用户信息/查询/片段及已知协议路径；第三方代理、相似域名、未知路径继续按其配置读取目录，不做跨主机回退。旧管理接口显示的目录地址也与实际请求一致。

仅新建的OpenCode Go API凭据初始化 `User-Agent: TokenGo/<版本>` 与 `x-opencode-session: {{session_id}}`。不会改写已有Key、自定义头、按量或其他供应商。Responses客户端仍应提供显式稳定会话ID；本改动没有扩展匿名会话推导规则。

三个品牌图标为静态SVG，保留真实来源、内容哈希和许可证；已检查无脚本、事件处理、外部引用、image或foreignObject，并在实际构建页面核对加载。

## 验证

构建/测试全部在build-test独立目录 `/srv/build/tokengo-provider-presets-20261102/` 执行。Mac保留源码编辑、差异检查与Git管理。同步当前未提交修改及新增文件，核对逐文件SHA256；源码基线 `1491d08`。必要测试先在旧行为下失败，随后修复通过。

- server lib目录适配与边界：2项通过。
- provider_catalog：13项；provider_channels：26项；provider_pool：16项；sessions：6项，全部通过。
- web lib：175项全部通过。
- 共238项测试通过；Linux预览服务构建与正式优化的WASM页面构建均退出0。
- Chromium访问真实合成实例，无前端请求拦截：四个预设的地址/名称/认证选择、OAuth切回API、家族切换、手工地址保留、Escape关闭均通过。
- 桌面1440×1000与手机390×1000无横向溢出；四个图标加载正常。沿用现有桌面40px卡片密度，手机点击高度至少44px。

最初浏览器脚本把包含帮助文字的地址字段可访问名当成精确短标签，并套用手机44px标准到既有桌面40px控件，产生脚本断言失败；按实际DOM标签和既有桌面设计修正后通过，未为消除该失败改产品代码。先前失败DOM和截图保留。

代码测试通过后只补充图标许可及来源清单，核对代码哈希未变后重新打包页面，没有重复无关测试。全部日志、源码清单、浏览器报告、截图及Linux预览构建产物已取回Mac工作区的 `review/provider-presets-20261102/`。

公开目录只读探测：build-test上的OpenCode Go与按量目录分别200（28、78个模型）；同轮CommandGo读取和OpenDesign TLS握手超时。另一次Mac只读探测分别得到CommandGo目录200及OpenDesign未认证401。不能把这两种执行点的差异表述为全部上游业务可用。没有使用真实Key进行新套餐生成或额度验收。

## 交付状态

本次为已验证的本地源码改动和预览产物，未推送、未发布新版本、未部署线上，不改变前次已修复的Go渠道配置。Linux预览包包含调试服务端和优化页面，不含运行数据库、主密钥或生产凭据。后续发布应按仓库发行约定同步清单与发行资料。
