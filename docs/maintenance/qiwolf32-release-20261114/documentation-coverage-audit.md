# qiwolf.32 文档覆盖审计

2026-11-14，只读核对 qiwolf.31 标签至当前源码 7ce8f8747baa，并检查本轮待提交文档。覆盖清单来自 git diff --name-only v0.34.0-qiwolf.31..HEAD -- crates deploy .github，不以维护记录标题猜测产品变化。当前公开发行、精确 CI 与产物新验收仍待主发行流程填写，不在本审计预报成功。

## 对照矩阵

| 变更组/源码 | .32 版本说明与当前文档 | 证据与边界 |
| --- | --- | --- |
| billing查询/management、usage_time.sql、reporting_currency、usage_dashboard/usage_period/account UI | 用量区间汇总、模型分析、用户/Key/来源筛选、旧USD与新CNY完整纳入；identity-metering、指南、requirements | 用量专项收据；不改历史金额；既有50万条低于1秒性能门禁未绿 |
| audio_meter、routes/audio、billing_units、currency_units.sql | ASR时长与Token/request/quantity分开计价；schema47和用途优先 | 价格配置不代表TTS/图像/视频自动计量接口已实现 |
| billing_price_lifecycle、routes/billing、Web billing | 版本化修改、启停/恢复、删除归档；不回旧价，保留旧快照 | 三类生命周期测试与专项UI收据，历史实费不重写 |
| routes/model_limits、pricing_options、connection_parameters/provider_keys | 同来源Key目录、管理员调整、有效身份缓存、来源时间、unknown不猜 | 目录是供应方声明，非付费硬窗口实测；OAuth不误用普通secret |
| db/billing、billing_units、management、billing_writer、accounts/managed_users、passthrough、vector_compat | 取消所有金额门槛/现金hold/冻结；已知全额、未知null；旧hold恢复 | identity-metering、account-login、passthrough、共享/向量指南已同步；权限及RPM/TPM仍生效 |
| gateway、provider/key/policy/virtual UI | 当前配置顺序/Key策略和可用性选源、去历史pin/active/affinity、忙源试其余Key；400/429开关及已提交流不重放 | protocols、Key池、logical-routing/matrix、共享模型与双语指南；源窗口与TPM独立保留 |
| protocol_config、routes/admin、channel_protocol_patch、参数页/Key导航 | 容量不改身份，协议hash+generation事务CAS、选中来源参数直达 | runtime-patch-audit 保留已部署主体与新文案hash边界；旧客户端无hash不冒充双版本保护 |
| gateway流式结构诊断 | 有限白名单shape、终态/token数；不记正文/头/密钥 | runtime-patch-audit；不改变流式透传或费用规则 |
| Web console-design/index/style/usage-dashboard CSS | 手机价格字段与操作、菜单视口、看板筛选布局 | 实际WASM Chromium/WebKit覆盖，不是真机Safari操作 |
| deploy/package.py/test_package/test_manage、db/schema/migrate-only | package schema47、46/47迁移SQL及清单；升级停服、整组备份与回退 | deployment/container及中英指南；不沿用schema45对旧程序可写兼容结论 |
| server各协议/权限/来源/工具集成测试、Web单测 | 维护真实参数/权限/窗口/流提交边界，修旧预算/pin合同断言 | 这些测试修改不逐一冒充新产品功能；本版exact workspace/CI结果待独立填写 |

扫描源码文件数量：{"crates/server/src/": 27, "crates/server/tests/": 57, "crates/web/": 21, "deploy/": 3, ".github/": 0}。未发现未被上述功能组覆盖的产品源码或打包变更；测试与样式文件按其所属行为分组，不逐文件重复发行条目。历史 session-route-isolation 是中间版本，已在当前合同说明其锁源行为被替代，不删改历史收据。

## 当前合同与历史材料

二次检查发现 requirements-status 仍自称 .18/schema41，且表格宣称 microUSD 预算、旧用户亲和；已更新为本版pending、只读费用/生命周期、目录发现和手机范围。vector-fallback 的预算执行表述也已替换为实际RPM/TPM/并发检查。当前 README、指南、身份账务、Key池、协议、共享路由及部署不再要求金融预算或历史会话锁源。时间预算、thinking预算与数值误差预算是独立概念，保留。

旧 CHANGELOG、验收清单和维护收据包含当时的预算/亲和行为，入口已明确为历史且被2026-11-14合同替代。unified-user-and-quota-plan 是未实施权益方案，已加替代说明，不将其草案变成新功能。

## 链接与发布包文档隐私

本轮检查 .31→HEAD 的47份变更Markdown，相对文件链接全部存在；文档新修改的链接另复查。此检查验证文件路径，不代表外部URL可达或所有锚点/图片内容逐一人工验收。

package.py复制完整docs目录。对当前docs的812个文件做库存，对Markdown/JSON/TXT/LOG/Python/HTML/YAML/CSV文本按常见Key前缀、私钥头、JWT及URL userinfo模式扫描，候选0；输出仅记录文件/数量，不打印疑似值。此为有界静态检查，不等于凭据绝不可能存在：没有OCR所有历史截图，不把账号显示名、合成记录或已公开部署收据当作推理授权。源码、包内清单及公开资产仍须发行流程核对。

Mac docs下有未跟踪.DS_Store，不能将整目录未审查同步当作发布源码。应从已审查提交构建或核对包内docs库存，确认未跟踪review、机器缓存和私密材料没有混入；本审计不删除用户文件，不更改打包源码。
