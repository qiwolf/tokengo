# qiwolf.32 发行与公开产物验收

状态：已正式发行，独立公开产物验收通过。Cargo 0.34.0，schema47。固定源码为 `4ab9085fcb758038d3a02c6de031dec4e659b612`，标签和包内文档保持构建快照；本页及 Release 正文记录后补结果，不移动标签或替换附件。

## 本轮范围

完整功能见[版本说明](../../releases/v0.34.0-qiwolf.32.md)，逐项覆盖见[文档审计](../../maintenance/qiwolf32-release-20261114/documentation-coverage-audit.md)。纳入 qiwolf.31 后的用量与币种、价格生命周期和类型配置、来源上限发现、取消本地金额门槛与会话锁源、移动端、来源参数版本保护及有限流式诊断；发行前另修复明细分页与全历史图表耦合。

## build-test 证据边界

代码在 Mac 编辑和管理 Git，执行在 build-test Linux x86_64。源码同步逐文件核对 SHA256，排除真实环境、凭据、依赖缓存、运维 review 与未跟踪文件。日志和产物已按批次取回 Mac 项目 review 目录。

| 检查 | 结果与范围 |
| --- | --- |
| 初轮完整 workspace | e0c2ea1：1742 通过、27 失败、15 忽略。失败分布于 9 个集成目标，均保留原日志；该轮不能称为全绿。 |
| 合同回归修正 | 将旧金额冻结、现金估算扣款、会话签发/固定来源、历史 Key affinity 断言改为用户已明确的新合同；保留权限、真实来源限制、RPM/TPM、提交后不重放和显式 400/429 开关。未删除或忽略测试。 |
| 相关服务端回归 | 初轮失败的 9 目标均已复测通过；后续两组已知实费累计断言复测为 proxy 135/0、shared_virtual_routes 28/0。其他新统计 HTTP 与只读事务测试另列，不重复相加。 |
| 新统计读取 | 独立只读事务版本：43 项 HTTP 与 32 项 usage_ 单元测试通过。覆盖管理员、当前 Key、本人门户、运维令牌、撤销、过滤、完整范围不受游标截断、字段投影及只读快照与主写入并行。 |
| Web | 225 项库测试通过；新 WASM 编译通过。管理员/普通用户、390/1440、Chromium/WebKit 共 8 场景 80 检查通过；独立统计加载、筛选竞态、分页、页大小、刷新与错误恢复均验证。首轮 QA 选择器歧义保留，修脚本后对同产物复测，未改产品迎合测试。 |
| 手机布局 | 同一份 WASM 的 Chromium/WebKit、320/390/430、短横屏/桌面、明暗主题共 20 场景 88 检查通过。两组浏览器报告逐文件产物哈希相同。不是实体 iPhone Safari 验收。 |
| Python 合同 | 初轮 MinerU 13、部署包装 11、容器配置 3 项通过；新增包装测试核对 schema47 的 43–47 SQL 原字节/哈希与缺失拒绝，并保留 schema45 包验证兼容。 |
| 初包预检 | e0c2ea1 的 Linux server/CLI/WASM 构建与 bundle verify 通过；原生和容器的隔离启动、schema47、Key 加密/重启持久化、Web 字节身份等通过。仅为初包 preflight，不替代最终提交的包及公开资产验收。 |
| 原规模性能 | 原 1M 用量分页各过滤最慢 8.477ms（原 300ms），12 并发与写入检查通过；完整百万行报告实测 6.280498s，其进行中的三次翻页为 6.925/5.219/4.090ms。50 万条个人门户最慢约 500ms（原 1s）。管理统计原 2160h 曾最慢 1.066161s 失败，减少单次扫描的重复分组查找后，168h/2160h 最慢为 898971/913401us，原 1s 门限通过；样本与计量断言不变。目录规模检查通过。按改动隔离复用已绿目标，不把重叠检查相加，也不以轻分页耗时冒充全图表分析耗时。 |

未新增全部供应商、全部模型或持续可用性测试。此前 DeepSeek fallback、GPT Sol/Luna、向量/重排的真实调用恢复属于各自提交和指定生产实例的既有证据，见[模型核实清单](../../provider-verification-checklist.md)及专项收据；不能冒充本次正式公开包新测。本次 GitHub 发行不会自动升级任意生产实例。

## 正式发行与独立公开验收

[精确 main CI](https://github.com/qiwolf/tokengo/actions/runs/43556308521)成功：Rust **1778 通过、0 失败、15 忽略**，Python 客户端 13 项、WASM check 和 5 项显式规模检查通过。规模检查与原 ignored 身份重叠，不累加为唯一 Rust 总数。该结果覆盖最终完整源码，不能与前述多轮局部通过数相加。

[发行流程](https://github.com/qiwolf/tokengo/actions/runs/43557334653)成功，[正式 Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.32)于 **2026-11-14 12:50:41（北京时间）**公开 9 个附件。annotated tag 对象 `ed245f8c88ae36330975e806d96aed2582bc5d28` 指向上述提交。原生包 SHA256 为 `9e2f89b6a66e53a86d90d69835bb72a14eb6fc1a7fe01ef0ae0c49bbc731c1b1`；离线镜像包 SHA256 为 `d1d6c7dba096d558c18b1c70e5c5ce0487d544aa7fa8be73f9e5ba5d181685d7`。

公开后在 build-test 独立完成匿名下载及运行验收，24.681 秒、exit0：9 个附件大小及 GitHub 官方 digest 一致；包内 checksums、**813 份文档的完整库存与字节**、Cargo.lock、manifest/schema47/43–47 SQL及45组合均与 tag 源码匹配，没有混入未跟踪 `.DS_Store`。公开 ELF/CLI、实际 HTTP Web/WASM 字节、schema47、Key 加密/重启持久化及容器非 root、只读根文件系统、重启与同卷重建均通过。

镜像为 `ghcr.io/qiwolf/tokengo@sha256:48bca4537466ab318bd0050328613a5e767d4529f49d964b18c835dc2a41195d`。空 DOCKER_CONFIG 匿名按 digest 拉取，并与离线镜像内容、两个精确 SHA 别名、OCI revision/linux/amd64 身份核对通过。Compose 只验证无 `.env` 的 config，未进行 Compose 部署。此次合成容器、卷及新增别名已清理；没有访问生产或调用真实模型。公开包的浏览器行为没有重跑，行为证据为前述同前端源码的 build-test 实际 WASM 28 场景，不能将公开 HTTP 字节校验说成公开包浏览器验收。

机器可读的附件摘要、CI 与边界见 [verification.json](verification.json)；已执行的独立 QA 脚本保存在 [qa/](qa/)。本地完整日志、9 个公开附件及执行收据分别保留在项目 `review/qiwolf32-release-20261114/` 和 `review/release-qiwolf32-20261114/public/`，未将二进制、登录材料或原始运维目录批量纳入 Git。
