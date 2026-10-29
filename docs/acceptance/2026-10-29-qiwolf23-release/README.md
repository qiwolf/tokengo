# qiwolf.23 生产验收与发行进度

2026-10-29，产品源码`5c6efa71da32803c6624f91515c01b323bc26c72`已完整部署到生产，Cargo0.34.0/schema42不变。生产验收通过；[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40884046534)与[发行流程](https://github.com/qiwolf/tokengo/actions/runs/40884478643)均成功，[正式Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.23)于2026-10-29T13:53:57Z公开9个资产；公开9资产匿名下载、GHCR及原生/容器独立验收通过。结构化脱敏结果见[receipt.json](receipt.json)。

## 构建与产物身份

Mac保留源码和Git，build-test完成最终源码同步与一致性核对，完整Rust工作区1570通过、0失败、9忽略；Python分别MinerU13、部署6、容器3通过。native release workspace构建、WASM check及发行包验证通过；339个包文件校验通过。本地原生归档SHA256为`6854bde81fa4a5f144d860b2b98bab0386e31dcb4c56767bb0f7e67b03b82cfc`，它与GitHub公开构建的资产摘要分开记录，公开元数据见[publication.json](publication.json)，实际下载核验见[独立审计](independent-final-audit.json)。

Web沿用此前已完成WASM release构建与真实产物浏览器验收的包；最终源码69个前端及共享文件与该构建逐字节一致。本轮最终源码执行WASM check，不声称重跑WASM release构建。此前173前端、26推理能力及3 HTTP事务回归共202项与完整工作区有重叠，不累加。

9项忽略仍保留各自边界：两项性能基准、子进程helper、外部registry、浏览器harness、三个SF手动入口和真实云到GPU回退。此前q22的真实供应商结果不冒充本轮重新实测；本轮浏览器检查也不替代全部供应商和全部协议组合。

## 生产部署与验收

完整native+web在入口门禁、请求/账本/账务预占排空后停服冷备并切换。切换期间schema与受保护业务表一致，运行配置一致，运行程序身份、Web资源回读及入口恢复通过，未强行终止在途请求。备份及回滚具体机器路径保留在本地交付记录，不写入公共文档。

首次准备因保留旧Web资产导致checksum集合不一致，在入口切换前停止；修正为逐项校验新包后，仅保留与旧版本相同的额外Web文件，扩展checksum并再次完整校验，r2部署完成。旧hash资产保留以供切换前已打开页面继续加载。

DSH选`gpt-6.1-terra`、Default，界面完成并返回预期回复。request109268、109269经channel27，均为status=ok、无错误；数据库HTTP状态字段为null，不据此编造HTTP200。当前直接上游none最小请求和生产客户端结果仅覆盖该精确来源身份与模型，不外推到其他账号、Sol、第三方接入、工具或多轮。

线上禁用临时连接57验证追加light及默认light一次保存、覆盖为high及默认high一次保存。覆盖草稿移除已选默认light时，界面显示已移除及必须重选的错误，点击保存后数据库仍为append light/default light。线上未捕获网络，不声称零HTTP请求；隔离真实WASM浏览器另已验证无效默认不发保存请求。临时连接已清理，模型请求数0；人工值保存成功不等于来源接受该参数。

## 回滚边界

已准备完整应用回滚及冷备，保留上线后请求和账务，不用升级前整库覆盖业务写入。旧程序不识别append对象，回滚须从最新已认证的有效档位导出转换为旧数组，并校验连接代次、目录及配置没有并发变化。DeepSeek append需先明确人工清除或转换，不能自动冻结后冒称行为等价。同连接多append事务和每连接generation仅加一次的模拟检查通过；没有执行生产实际回滚演练。

## 公开资产独立验收已通过

build-test对已公开9资产执行匿名下载、大小/GitHub digest与包内checksum核验，匿名按digest实际拉取GHCR，离线镜像双标签、无.env固定SHA Compose启动、非root/只读根、ready/WASM、重启及同卷重建通过；公开原生和容器的Key生命周期与权限检查通过。公开原生包SHA256为`7ad0569498f012a4274b935d12721d5f3441dd92cc1bbdef3d55aeee823da613`，镜像为`ghcr.io/qiwolf/tokengo@sha256:a619ae6dc1f8c79aa2a81860b5b1b7d6c668e3afd7a12cbda1868d755be36623`，与本地生产构建身份分开。

[总体收据](verification-q23.json)、[独立最终审计](independent-final-audit.json)、[原生运行](native-runtime-q23.json)、[容器Key检查](container-key-runtime-q23.json)、[镜像标签](registry-tags-q23.json)及[生命周期](image-smoke-q23.json)记录范围。公开验收未访问生产；生产DSH/UI使用前述独立证据。

公开包WASM包含来源、追加/覆盖及默认选项等预期UTF-8标记，见[静态身份收据](public-ui-identity-q23.json)。此项只证明已校验公开包的静态内容与源码身份，没有执行公开包浏览器交互，不能替代此前隔离真实WASM和生产UI操作证据。

验收helper的一个断言适配了省略空数组字段的既有序列化合同：`verified_levels`为空时可能省略，读取改为`cap.get('verified_levels', []) == []`。仅改任务独立helper，不修改产品源码、已发布包或标签；其最终哈希见[helper哈希](public-script-hashes.json)。验收没有删去非空数据检查，原生新库仍验证追加/覆盖与默认原子事务。

最终审计确认临时运行目录、测试容器、卷和原生进程均为0，任务镜像别名已清理，原有容器/卷保留，公开资产留存在任务缓存。真实OAuth用户登录、付费推理及全部供应商组合未由隔离运行覆盖。精确main CI与发行流程均匹配产品源码5c6efa71da32803c6624f91515c01b323bc26c72。后补结果维护main与Release页面，不移动已发布标签，也不追认此前包内文档包含后补证据。
