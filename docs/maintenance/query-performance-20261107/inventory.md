# 查询性能审计清单与实际覆盖

2026-11-07。以下风险表记录修复前 `1394c60` 的静态调查，不能把其中“建议/需测”误读成最终实现状态。当前已完成的修复、精确源身份及专项验收见[RESULT.md](RESULT.md)；最终覆盖状态见本文末尾。**源码有索引不等于实测达标，静态检查不等于每个接口已运行。** 生产查询计划及初次浏览器现场证据另在工作区 受控本地证据。

本清单覆盖 `crates/server/src/db` 模块、主要路由及直接SQL调用方、网关准入、协议会话、后台清理/探针。`sql-source-inventory.json` 提供源码SQL位置的保守词法索引，排除各文件首个 `cfg(test)` 之后内容，不能当编译器AST或穷尽运行覆盖率。各模块的迁移SQL也已查看主键、唯一键及已有索引。

## 已确认应修复的查询形态

|类别|基线位置/行为|风险与必要验证|
|---|---|---|
|模型评分|`db/scores.rs::for_channel` attempts JOIN requests，按requests时间过滤|生产计划实际全扫attempts。必须从requests时间窗驱动，用attempts.request_id索引；旧历史增长不应让2小时窗口变慢。核对样本、错误、延迟、归一化语义。|
|模型探索统计|`routes/admin.rs::model_routes_list` explored聚合|生产计划全扫requests并临时分组。应部分/覆盖索引；保持served_model原始分组口径。|
|模型终身次数|`db/scores.rs::total_calls`逐行遍历model_invocations|本轮370,212行544毫秒且覆盖索引，仍与终身历史线性相关。若改汇总，必须按request_id与归一化名称去重、保留裁剪后累计、涵盖新增attempt。|
|管理员总览|`db/requests.rs::stats_since` attempts JOIN requests按时间过滤|与scores同类潜在反向连接；需EXPLAIN证实时间索引驱动，不能只改scores遗漏此处。requests窗口本身一次扫描合并多维，旧legacy函数仅测试对照。|
|默认路由策略|`db/policy.rs::rank`无条件scores::all|sequence/balanced/idle不需要评分时也扫描历史；仅评分策略或命中的探索分支加载。探索查unused用COUNT仅判断0，应EXISTS短路。保持探索5%及策略排序。|
|供应商目录|`db/provider_channels.rs::list`逐供应商重建路由|同一响应复用渠道/路由快照，避免供应商数量乘以全目录成本。不得把授权快照跨请求缓存后用于派发。|
|账务对象|`db/billing_management.rs::objects`从历史ledger补已删除渠道名|生产全扫ledger分组；目录复用及按channel/id访问。必须保留历史渠道快照。|
|账本页|`routes/billing.rs::admin_usage`先usage再next再enrich|三次objects构建，页面另读objects。一个响应只构建一次，分页应按LIMIT+1确定下一页，当前有效/历史过滤前后语义不变。|
|维护信息|`db/maintenance.rs::info` COUNT(*),MIN(ts_ms)合在一条|组合聚合会逐条遍历；拆count与min，可让count走较小覆盖索引/COUNT快路径，min用索引首项。绝对数量仍准确。|

## 其他查询类别及边界

|类别|访问方式/基线依据|边界与测试要求|
|---|---|---|
|最近调用日志|`db/requests.rs::recent` ts_ms DESC,id DESC LIMIT，既有idx_requests_ts|顶层最多1000（路由限制须验）；逐行主键子查询标签，另全渠道标签快照。相同毫秒排序稳定；请求历史百万条、LIMIT12及1000都测。|
|请求详情/attempts|`db/requests.rs::detail` request主键、attempts_request索引|单请求重试数量无独立分页；正常重试上限与畸形历史fixture都测，不用误删旧数据控制性能。|
|个人总览recent/detail|`db/account_overview.rs` owner、(ts,id)游标；requests_owner_recent|跨用户噪音已有测试。需补同一重度owner百万历史、跨页游标（同毫秒、非法/其他owner游标）。不能为性能移除owner gate。|
|个人统计|`db/account_overview.rs::stats_since`同owner窗口aggregate四次（total/model/client/kind）|有owner,time索引，仍四遍读窗口、客户端维度可能高基数。需测单用户大窗口；必要时像管理员统计一次读合并。统计本身准确遍历窗口是有意成本，不能以截断样本伪装加速。|
|个人历史汇总|`routes/accounts.rs::portal`（`/api/account/portal`）对owner全历史ledger count(distinct request_id),sum|billing_ledger_user限定owner，仍终身线性。必须把此端点列入规模门禁；汇总优化保留未知用量、未定价、重试、debit及空值口径。|
|预算/预占/结算|`db/billing.rs`预算主键，ledger状态，allocation外键；`billing_writer`|账本agent审计。必须测pending/reserved大样本、终结幂等、多scope、恢复；不能异步后置额度判断产生超支窗口。|
|用户/Key列表|`db/identity.rs::users/keys`先ID再逐条主键装配，含祖先链与软删设置|N+1但以配置量增长，不以调用历史增长。1000用户/10000Key及深祖先链场景测；分页若改需兼容API。权限变更立即生效。|
|鉴权|`gateway_keys` hash/Key主键；accounts session_digest主键；ops public_id唯一键|每次请求的存在/停用/撤销/父Key状态必须实时复核。不可用TTL缓存跳过撤权。账号root找role已有部分唯一索引。|
|账号会话|`accounts::create_session`清过期session，按account保留10；panel_sessions过期索引|account_sessions缺独立expires索引但正常总数10×账号数；应测试大量账号的过期清理。登录需密码哈希，单独区分KDF成本。|
|operations审计|`routes/operations.rs` id游标ORDER BY id DESC LIMIT|LIMIT+1上限、日志脱敏、下页语义；OR可选游标需计划验按id seek而非每页从头扫描。|
|operations token|public_id唯一/active索引，列表可选include_revoked|总量以配置/审计量增长；创建有效token容量限制，撤销历史仍保留。|
|channels目录|`db/channels.rs::list`父列表+每渠道headers/models两次查询|N+1配置规模。`for_model`先扫channel_models做Rust精确归一，后装配匹配渠道；不是历史请求全扫。100/1000渠道×10映射测。不能将SQL lower/trim等同Unicode/后缀归一。|
|模型/策略/冷却|settings主键；model_cooldowns主键channel,model,upstream；policy::rows按model|按model查询缺model前缀索引，可能每模型扫全冷却配置；活跃冷却全表扫以配置量增长。建议model索引并测1000模型。faults按channel+revision复核。|
|模型检测状态|admin按model_name查type_checks ORDER BY next_at DESC LIMIT1|已有主键channel_id,model_name，不支持model优先；建议(model_name,next_at DESC)，用扫描计划门禁。|
|服务透传|`db/passthrough.rs::list`父列表+headers/acl子查询|PK route前缀，N+1配置量；路由匹配目录读取不应关联ledger历史。配置1000服务、ACL1000条测，不暴露header cipher。|
|用户虚拟模型|owner,normalized_name唯一；targets/windows按virtual_id,position主键|每条targets<=16、windows<=64，父模型数量无硬上限；列表N+1。私有owner权限、名称归一兼容回退需保留。|
|共享虚拟模型|normalized_name唯一、ID主键、config_json全列表|配置目录无分页，与历史请求解耦；跨私有名称冲突检查必须保留。|
|目录/协议/推理能力|channel_catalog、channel_reasoning、settings点查|按channel/model主键/唯一键；请求内重复fingerprint、目录构建可能N+1，配置规模测试。外网catalog更新延迟须独立于本地查询测量。|
|OAuth账号与绑定|account/channel主键与issuer/client/subject唯一；设备pending过期索引|pending最多32但状态count应测历史已完成积累；每次登录增历史pending，status='pending'索引可按规模需求补。网络刷新不得持DB锁跨await。|
|RPM/TPM准入|`admission` quota_events_scope_window与expiration索引|窗口匹配后逐行tokens累加，过期DELETE一次清空；仅一分钟事件但高吞吐可大。测10万/百万过期、活跃并发，不改精确限额。|
|协议会话/opaque所有权|key/owner,digest主键；last_used与owner_recent索引|新session做COUNT容量检查；artifact有全局容量上限；owner cold LRU每次最多1024、保留最近500。已有20k owner测试。cleanup却一次DELETE全部过期，恢复时大量TTL过期需测长锁，必要分批且保持立即拒绝过期输入。|
|保留清理|`db/maintenance::prune_batch`按ts索引、LIMIT2000；janitor批间释放锁并sleep20ms|验证尝试级FK级联，不碰billing历史、model_invocations终身计数。每run最多批数有界；巨大attempt/单request也会放大批成本。|
|磁盘回收|maintenance::reclaim_space，小库<=200MiB直接VACUUM，大库64轮增量+checkpoint|明确属于运维写操作，不能与只读页面基准混为一谈；大库不自动整库重写。checkpoint(TRUNCATE)可能busy，需记录未回收边界。|
|启动恢复/迁移|recover_interrupted、billing恢复、历史汇总backfill、密文迁移|启动可合法单次扫描/写历史，不能搬到每请求；需在百万数据升级测试记录迁移耗时/失败回滚/重启幂等。|
|自定义探针/后台检测|settings custom_probe前缀、channels路由目录，type_checks复核|配置量驱动；禁止在DB锁内等待外网。探针不应让用户权限/数据库状态长时间快照化。|

`requests::prune_before` 是无界DELETE便利函数，但本轮引用搜索只见测试使用；真实janitor使用prune_batch。因此不能把测试便利函数称为线上无界清理根因。

## 门禁设计建议与实际采用范围

本节原始建议用于设计门禁，不代表逐项已实施。实际采用的管理fixture为各100万/10万 requests、attempts、ledger（约3.73GB/389.5MB）；个人fixture为50万owner行+1000其他owner（约1.123GB）；配置规模最大为560用户、1330Key、350渠道。下文1000用户/10000Key等数值是进一步扩展建议，**本次未测**。只在build-test合成数据，不拷贝生产凭据、患者/个人数据、请求原文。生产当前1.60GB是体积而非唯一规模参数。

1. 固定时钟、种子，创建100万requests、120万attempts、120万ledger，旧历史占99%、两小时窗口占1%；另有6万/24小时重度owner窗口，测试非均匀分布。可加512—1024字节合成error/source_snapshot使文件体积接近生产，避免百万窄行误判。
2. baseline与fixed复用同一只含合成数据的数据库副本。分别记录SQL计划、VM step、返回行数、P50/P95/max、文件页数，结果统计与逐条legacy oracle相等；计划门禁比固定毫秒阈值更适合拦截旧历史全扫。不得只测内存库掩盖页缓存/磁盘影响。
3. 查询矩阵包括所有上表类别的代表调用，尤其空结果、稀有过滤、深页游标、已删除对象、已撤销Key、旧账、未知用量、重复模型拼写、多次重试、多个owner与高基数client。
4. 用真实loopback HTTP启动隔离实例、模拟上游。模型/总览/账本每5秒模拟轮询，同时发日志/配置轻请求以及真实模拟chat/embedding，测轻请求P95与总请求P95、失败数、费用/权限一致性。单独HTTP跑快不能证明共享锁争用解决。
5. 验证10倍旧历史而活跃窗口不变时，窗口查询VM步数/耗时增长受控；终身统计应按已定义增量机制运行，而非每次线性扫历史。记录迁移backfill一次成本。
6. 全套必要语义测试通过后只补新失败、环境变化或未覆盖项，不反复跑无关全套。UI实测最终发布版本、暖页面切换与首次加载分开报告；生产健康200不等于页面性能验收。

## 审计结论使用限制

此清单确认了访问形态和风险，不声称全部类别都存在性能缺陷。主代理应给每类补充“已测/已修复/合理线性成本/未覆盖”的最终状态及证据路径。没有实测的类别不得写成“全部查询通过”；配置量扩展、迁移、清理和外网请求有不同边界。

### 本次补充修复与已完成专项

- `policy::rank`：非评分策略不加载历史评分；探索unused用EXISTS短路；仅探索确需低评分池时再加载评分。3个maintenance/policy回归中，两项用真实函数的SQLite VM操作预算而非毫秒阈值拦截历史逐行扫描。
- `maintenance::info`：拆精确COUNT与MIN，以保留SQLite快路径，返回口径不变。
- `protocol_sessions`：expired artifact单次最多删1024；session候选最多1024，且有子项者不物理删除，避免ON DELETE CASCADE无界清理。读、claim和proof明确检查TTL；session `issued_ms` epoch（migration45默认0兼容旧记录）防止同名session重建后借用前一epoch残留artifact。实际重新签发可更新过期/旧epoch摘要的issued_ms；普通重签发仍保留原issue时间。物理数量未达容量时用COUNT快路径，到容量才核对有效记录数量。
- 新增协议三项回归：10k单owner子项清理预算、尚未清理的expired proof拒绝、同owner/pin重用epoch防复活及实际重签发。保留既有500条LRU、20k池、回滚和owner隔离语义；历史测试fixture显式重建早于artifact签发的owner epoch。

上述补充功能已由acceptance-server-lib与单独百万容量专项验证；基础workspace（1636 passed）、Python（13项）、正式二进制与旧ELF升级/回退smoke也已通过。仅billing_management.rs的最终delta已通过账务10项、配置目录、百万/10万及真实HTTP并发重验，最终正式制品与真实新旧ELF升级/回退smoke也通过，总退出码0。独立源码review无阻塞项。代码已本地提交生产配套构建（非公开标签源码），未推送；用户明确授权后已生产上线schema45/cad0bc...并完成Chrome暖SPA切页，91响应全部200，详见[验收记录](RESULT.md)与生产验收（受控本地证据）。

## 实际覆盖矩阵（acceptance批次）

|域|已完成证据与范围|仍未覆盖/成本边界|
|---|---|---|
|管理模型、路由、评分、累计、探索|百万/10万宽历史的真实Router路径、时间驱动计划、归一与去重单测；最终delta百万模型路由最大34.378ms|配置无限扩展未测；独立外网探测/推理链路未用本地查询耗时冒充|
|供应商、渠道、冷却、策略目录|百万背景下目录端点；lib语义/快照测试|每个外部供应商响应速度未测；配置10倍目录专项不等于所有路由组合逐一穷举|
|管理员统计|1/24/168小时端点；50万重度owner的1/24/168/2160小时精确统计|长窗口随命中行数线性；50万owner管理员最大998.743ms接近1秒，无更大窗口保证|
|日志与请求详情|百万/10万日志、详情端点及并发日志；owner游标与隔离语义测试|并非每条百万历史详情逐一读取；异常无限重试单条详情上限未专项压测|
|当前对象、历史目录、预算|56/133/35及十倍560/1330/350配置，含停用/删除/过期/父Key；当前规模门禁，十倍record-only；最终delta历史渠道SQL固定7distinct的100/100000行VM均434，百万对象目录最大2.511ms|1000用户/10000Key建议规模未测；十倍managed-users最大304.377ms超过300ms记录线，不能说十倍全达标；目录仍随distinct渠道数量增长|
|账本筛选、分页、历史保存|百万/10万有效、历史、全部及user/channel/model组合过滤、返回条数和next游标oracle；最终delta账务10项通过；百万历史账本最大6.944ms，全部4.524ms|未穷举所有过滤组合；稀有模型分布、distinct渠道增长等仍有数据分布成本|
|个人portal与统计|50万单owner宽历史与1000其他owner；portal、1/24/168/2160小时统计、日志；结果oracle|全历史费用/长窗统计仍与owner历史量线性；未测百万单owner|
|真实共享锁并发|每轮12路loopback HTTP、3轮；模型/日志/有效账本与真实DB写入竞争；最终delta百万最大136.728ms，写入最大22.287ms|生产同时7天统计时模型仍最高826.5ms，保留共享锁争用边界；非长期压测/浏览器INP|
|策略、准入、鉴权与撤权|594项lib批次包含策略/配额/身份行为，配置专项批量Key context|不能据此声称百万活跃quota事件、所有网络登录/OAuth路径都有独立计时|
|维护信息、保留清理|真实info VM预算回归，prune批次/边界/级联、保留期语义lib测试|未做生产清理故障注入或百万旧记录删除耗时专项；不把空间回收当只读查询|
|协议TTL、LRU、所有权、容量|10k单owner批次上界、既有20k LRU；百万活跃满载拒绝、旧epoch新签发、过期积压允许；proof/回滚语义|满载精确计数在百万上限内线性，最大290.26ms；不保证无限规模成本|
|迁移、重开、晚到写入|合成新旧ELF升级/回退及晚写刷新smoke通过；授权后生产schema44→45完成，备份与逐原业务列投影哈希守恒，运行制品cad0bc...|维护期间503与数分钟离线检查实际发生，非零停机；生产未执行灾难恢复或故障注入|
|生产Chrome暖SPA切页|供应商/模型/用户/额度/有效历史全部账本/24h与7d总览实际切换，91响应200；模型12样本81.9—826.5ms、active60.2—70.5ms、history384.3ms、all62.2ms、24h204.6—298.8ms、7d798.8ms|未清缓存，非冷首开/INP；前后自然流量不同，不能算受控提速倍数；短窗132请求130ok/0failed/2running不等于长期稳定性|
|透传、虚拟模型、OAuth、探针、会话配置|访问方式/主键/索引/界限已静态审查；相关现有lib/integration包含于基础workspace1636项通过|不声称每种配置组合、每个外部网络交互都做过规模性能测试|

总体界限：SQL词法索引的51文件/610处匹配用于找代码，**不是610条独立SQL的逐条运行验收**。配置目录与容量测试已经明确规模、语义oracle及门槛；未测项保留在右列，不用“所有查询通过”代替证据。
