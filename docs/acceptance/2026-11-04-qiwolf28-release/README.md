# qiwolf.28 发行与生产验收

精确发行源码b427946929d4157cc04576bd92046270d4248b11，行为提交23b26e9。先完成本地验证/配套包、生产升级、真实页面及调用验收，再推送main与不可变标签v0.34.0-qiwolf.28。精确main CI、正式发行及公开产物独立验收均通过。

## 已核实范围

build-test透传阶段服务端lib562通过/2项原有忽略、routes28、passthrough8、transport9；最新整合Web182、Key相关managed_users24/key_label1和最终disabled预算API通过，重叠结果不相加。三项旧行为红测分别证明旧迁移、路径记录和详情显示失败。完整源码manifest、最新整合优化WASM28资产、native release、打包及逐文件校验均通过并取回本地；最终5个Key修复文件hash与配套源一致。

schema42→43仅增加两个nullable TEXT路径列，无默认、无回填；生产旧请求全部原有列、attempts、旧迁移记录、业务数据及运行配置保持，历史新路径均NULL。程序与面板配套切换，所有Web资源HTTP字节校验通过。隔离合成库另实测旧q27发行程序在43启动、读旧/写新/读新和完整性检查通过；没有把它写成生产回退演练。

真实Chrome透传详情展示当前服务名、客户端入口及实际转发接口，无内部透传标识。新记录路径取实际调用且不存query，历史缺值由API与Web回归证明显示未记录，不推算旧接口。普通LLM虚拟入口、实际模型及接口合同保留。

同一已登录Chrome会话、无节流的热态单次对照中，Key页面网络关键链由8请求约4.82秒降至1个Key列表请求约57毫秒；原有Key完整显示，说明文字与相邻内容左边缘一致，16px间距已核实。此为一次网络关键链对照，不是冷缓存、点击端到端、所有用户或p95性能结论；未修改任何实际Key或额度。

升级前后六项现有调用套件均通过业务合同及账本：人脸、CLIP文本/图片、OCR、向量与原有本地聊天。另有两次独立CLIP调用用于真实UI见证，总计14次实调，API零重试；它们不混入6+6套件，第一次因最近列表被自然流量挤出未截到，第二次实际打开详情完成页面验收。新四项透传路径已核对，两项LLM路径及旧记录保持NULL；不公开实际路径、账本ID、真实Key备注或原始截图。

## 正式发行

[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/41685780070)首次成功：完整Rust1591通过/0失败/9忽略、MinerU Python13和WASM检查通过，与本地定向不重复累加。[发行流程](https://github.com/qiwolf/tokengo/actions/runs/41685779973)成功；[正式Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.28)于北京时间2026-11-04 01:51:18（UTC 2026-11-03T17:51:18Z）首次公开9资产。

九资产匿名下载、GitHub官方digest、精确tag源码、包内全部checksums/文档、真实匿名GHCR按digest拉取、离线双tag及原生/容器/Key/Compose启动、重启、同卷重建和清理独立验收通过。fresh schema43共60表，两path列为无默认nullable TEXT；包内迁移SQL与精确Git源码SHA相同，manifest四字段匹配。公开QA没有执行既有库升级、访问生产或新增原厂推理，临时进程/容器/卷及目录均已清理。公开WASM只检查静态身份和三项文案，真实浏览器行为来自部署的build-test配套包。

公开原生包SHA256：2ebff0f94e9739dbbba970ee893fff8cf4d89f49f19faab211d9bb67e77d409b；生产部署的本地配套包SHA256：d5ebd2f99e9ef37101f4a9ae6f46ded8de52b8f12f59f82a8496c4740ce71b89。两者均绑定相同源码，分别记录，不混称同一二进制。镜像：ghcr.io/qiwolf/tokengo@sha256:9d58a72e71192f07e6e1388857d2ae5c0bb94bb90f50678c59e13a67df03c588。完整安全摘要及九资产digest见[evidence.json](evidence.json)。标签与已发布资产保持不可变，包内候选文档是构建快照，最终收据维护于main与Release页面。

本轮不改变转发/鉴权/路由/账务分类、LLM模型归因、现有额度或历史成功率口径。当前服务名不是历史快照，历史无记录不猜测。既有模型能力沿用注明版本的证据，不冒称本轮原厂新测。公开WASM静态验收与生产真实浏览器行为分别记证据。
