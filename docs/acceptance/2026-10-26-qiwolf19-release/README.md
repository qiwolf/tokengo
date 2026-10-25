# qiwolf.19 发行与性能验收

2026-10-26（北京时间）。[正式发行](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.19)，源码 [`9a2276d96f6a5656545e4eb025947b0577c0167c`](https://github.com/qiwolf/tokengo/commit/9a2276d96f6a5656545e4eb025947b0577c0167c)；[精确 main CI](https://github.com/qiwolf/tokengo/actions/runs/40220762100)及[发行流程](https://github.com/qiwolf/tokengo/actions/runs/40221164948)成功。Cargo0.34.0/schema41不变。

## 性能与结果一致性

build-test 的实际 HTTP handler fixture：9供应方、35渠道、47映射、60k请求，每接口20轮。P95分别为模型与路由 **104.39ms**、总览统计 **123.07ms**、渠道列表 **11.28ms**，均低于300ms；权限拒绝和当前配置变更可见性通过。

复杂60k请求/80k尝试场景逐字段对照旧实现：旧单轮3275.56ms，新20轮P50 167.59ms/P95 **169.83ms**；累计调用60k行P95 **18.44ms**。旧值不是P95。保留历史NULL、重试归属、实际模型归类、未知用量、Top50并列、平均舍入和整数溢出语义。

现有实例在Mac Chrome、已有登录、局域网、暖SPA切换下完成三轮。下表是点击至控件恢复及实际内容确认的观测上界，包含自动化开销，**不是首绘时间或浏览器P95**。

| 模块 | 三轮观测（ms） | 最大值（ms） |
| --- | --- | --- |
| 运行总览 | 717 / 704 / 721 | 721 |
| 模型与路由 | 574 / 553 / 564 | 574 |
| 探针 | 550 / 582 / 548 | 582 |

三轮均在1秒以内；另两次预观测为774ms/582ms，也在1秒内。探针页当前为空，确认的是全部接入资料就绪后的空状态，未启动上游探针。Mac仅用于真实浏览器交互；编译、Rust/Python测试及独立公开包运行均在build-test执行。结果不承诺所有网络、冷启动、数据规模或供应方达到相同值。

## 测试范围

最终发行提交的GitHub完整工作区 **1377 passed / 0 failed / 5 ignored**，WASM检查及发行构建成功，见[CI汇总](github-main-ci-summary.json)。本机维护的首轮build-test完整工作区为1373/0/5；后续最终统计模块20项、排序预览1项、release基准2项分别通过，不与完整工作区总数累加。Web147项、Trunk发布构建、Python deploy6/container3/MinerU13项通过。

## 公开资产独立验收

9附件全部匿名下载，逐项核对GitHub大小和SHA256，再验证原生包内文件清单及源码提交。可信部署助手与该不可变源码提交的GitHub原文逐字节一致。

原生包在隔离loopback状态下通过readyz、CLI、schema41、integrity/FK、HTML和完整WASM字节比对、会话/机器凭据隔离及可恢复Key生命周期；仅使用临时合成凭据，未访问生产。容器使用空Docker配置匿名按digest真实pull，核对源提交、平台及image ID，通过非root、只读根、鉴权、重启及同卷重建。

不可变镜像：`ghcr.io/qiwolf/tokengo@sha256:e8bc13cca3e2957bb46378a5135e05635ec5e4736839419c5211ac76e36b8b2b`。版本tag、sha tag和latest在验收时均指向该摘要，latest以后可变化。离线镜像archive经过校验，未另作docker load或Compose启动；运行验收针对digest拉取镜像。

[独立总结果](verification.json)、[最终复核](independent-final-audit.json)、[原生运行](native-runtime.json)、[镜像身份](registry-manifest.json)。该次临时原生进程/目录、两个容器与一个卷均已清理并再次只读核实；下载资产与digest镜像保留供复核。

| 附件 | SHA256 |
| --- | --- |
| `compose.yaml` | `de0d464c6b1a396af256f91e9694eb1326e6c86bb83bb6b5692f9ea117c61c6a` |
| `container-deployment.md` | `0b244bfccf77a36ef7a5160b9386dc7909dfd81e21905d3e06929ed5bb094ff7` |
| `container.env.example` | `5719f373c3e78bf5b75a9eac193af05b9e96c1c9dff18218d54e320d3c0caed4` |
| `image-manifest.json` | `c574321e9c4b4f63897f75aa378a5b130768c234ef36482a6b68bfac0d89ffce` |
| `image-SHA256SUMS` | `058142f591ac007db0b07087a8e206373bf4e2814354f1f8c5f7716ed9a2023d` |
| `registry-image.txt` | `cc021ae0aa5ce13ea582068fea5af7adabbcab44df448a0eff86e3e0bc56c607` |
| `tokengo-9a2276d96f6a-linux-x86_64.tar.gz` | `9e60f2a7e59f577aa4f159ce7d4cc3851eb868d6c3aa27d3c698d4257dc7b50f` |
| `tokengo-9a2276d96f6a-linux-x86_64.tar.gz.sha256` | `87d13490f52119257afb513a6ec3613f34c2a0874755d4e70560270527cfb769` |
| `tokengo-9a2276d96f6a5656545e4eb025947b0577c0167c-linux-amd64.tar.gz` | `0ea5b0ecd8567e68f1b7a787af947055e6a77df38849fafe5eaadffbddc5d51e` |

## 现有实例与文档边界

现有237实例已更新至同一提交的build-test私有构建，服务端及24个页面文件摘要已核对，入口恢复；原业务数据、受保护配置及运行环境保留，schema41和60张表未变。这与上文公开附件验收是两组证据，不宣称私有构建与公开附件字节相同，也不替用户作个人体验确认。

配置见[三层负载策略](../../provider-key-pools.md)、[本地节点轮询](../../maintenance/local-vector-balancing.md)、[共享虚拟模型](../../shared-virtual-models.md)、[向量回退](../../vector-fallback.md)。兼容诊断仅用于主动排错，不阻塞保存、正常调用和配置回退。

**超时说明勘误：**包内早期文字将运行时读取既存虚拟名覆盖误写为当前管理界面/API可配置。实际可操作的是“系统设置→请求等待时间”的全局继承；模型路由编辑器和PUT只写已接入真实逻辑模型。目标节点轮询仍有效，但目标超时不会自动应用到虚拟入口。main配置文档和Release页已更正；不新增配置功能、不移动标签或替换已发布资产。标签/包内文档是构建快照，后补结果维护在main和Release页。
