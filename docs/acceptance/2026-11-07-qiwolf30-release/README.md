# qiwolf.30 发行与验收

**已正式发布并完成独立公开产物验收。** 发行源码为 `97c77379df0d973c3c1a15b6c0a06b6e35f10b32`，schema45；北京时间2026-11-07 18:48:03首次公开9资产。[Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.30)与[结构化收据](evidence.json)记录身份和摘要。

## 代码与构建

[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/42363611185)通过：原生工作区1638通过、0失败、15忽略，Python合同13、WASM及大数据量/并发/重度账号/配置目录和百万artifact容量门禁通过。与先行基础1636和最后目录补丁10项是重叠批次，不合并计数。

同一公开提交另在build-test从干净源码构建服务端、CLI和优化WASM，749个受控输入哈希一致，493.786秒完成并通过包校验。[正式发行工作流](https://github.com/qiwolf/tokengo/actions/runs/42364375957)独立完成同提交配套包、归档/架构、容器生命周期及GHCR发布。

## 正式资产与运行

9资产由GitHub官方下载，全部长度及官方SHA256匹配；大资产的8个分段均为严格HTTP206/Content-Range，拼接后再校验完整摘要。build-test核对原生包全部checksums、精确源码文档及schema45：保留43/44源SQL，45两part与额外LF组合的路径/字节/摘要一致。

公开原生包SHA256：`e67ecd885cf88719497ddbf5336757f30edeb194a6f4e190548ea3fb4666d7db`。实际匿名拉取镜像：`ghcr.io/qiwolf/tokengo@sha256:20755b0b4ad992716f83c2075ecbb6b092207fc83e5eadbff5b86e6dee239a89`。

公开程序离线migrate-only为62表；正常原生/容器运行后为63表，额外channel_capability_checks按发布源码DDL、列及FK精确校验。13个新增索引、3个dirty触发器、3个派生表和epoch默认0通过。CLI、全部Web HTTP字节、匿名owner401、Key密文及重启持久化通过；容器二进制/Web与公开原生包逐项相同，Key跨重启与同卷重建通过。离线镜像双SHA别名与匿名GHCR实际拉取同image ID。

临时原生状态、任务容器/卷及新增镜像别名均清理，原有对象保留。Compose只实际验证 `config --env-file /dev/null`，**没有执行Compose部署**。公开QA没有访问生产、调用模型或执行浏览器行为；这些检查不冒充新的原厂能力、生产回退或长期负载验收。

## 生产配套与容量边界

先行生产配套已完成停服备份、离线业务投影守恒与91个既有Chrome暖SPA响应全200；不是零停机。其服务器源输入与公开修复一致，但部署包和公开构建分别记账，不声称字节相同。本次公开发行没有重复生产切换。生产模型12样本81.9—826.5ms，峰值与7天统计争用共享锁；24小时204.6—298.8ms，历史账本384.3ms。

50万重度fixture管理员长窗998.743ms接近1秒门槛；十倍配置用户目录304.377ms属record-only。精确长窗统计仍随命中行数增长，不能外推任意规模或长期P95。既有原厂型号、协议、账号和工具证据按原日期范围沿用；本轮没有新原厂能力验收，见[兼容清单](../../provider-verification-checklist.md)与[查询摘要](../../maintenance/query-performance-20261107/RESULT.md)。

标签和下载包保留发布时的源码/文档快照；后补收据更新main与Release正文，不移动tag或替换资产。公开材料不含内部路径、真实Key备注、原始账目或生产截图。
