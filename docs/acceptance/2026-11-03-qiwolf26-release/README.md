# qiwolf.26 发行与237配套升级收据

2026-11-03（北京时间）。**已正式发行、公开资产独立验收通过，237配套程序与面板已升级。** 发行源码`c93975b5fe52655c442255fb72cb5eb12fb3bb26`包含产品修复`4292682`；Cargo0.34.0/schema42不变。结构化摘要见[evidence.json](evidence.json)。

## 构建与源码

Mac保留源码和Git；本地发行构建在build-test隔离副本执行，逐文件核对精确发行树。原生release构建、打包及manage.py verify均退出0；全部日志与配套包已回Mac。部署所用本地包SHA256为`c1cb4d05edf1a6640deda72e963be41ca80007c4a8b75e7008063bad3542d27d`，包内manifest为上述精确源码。Web沿用代码输入完全一致的优化构建，28个文件字节hash逐一匹配；26项路由集成和177项Web单元已在build-test通过。旧行为2项服务端与1项Web断言失败，以及首次漏同步sources.json的失败/补齐后成功收据均保留，见[修复说明](../../maintenance/passthrough-label-20261103/README.md)。

[精确main CI 41637257354](https://github.com/qiwolf/tokengo/actions/runs/41637257354)已成功：完整Rust workspace **1581通过、0失败、9忽略**，MinerU Python13通过，WASM check通过。该全量在GitHub执行，不冒称build-test另跑全量，也不与203项定向结果重复累加。

## 237升级与线上核验

237已从既有q23配套升级到c939程序与面板。升级前按零活跃请求、零待结算、零预留排空，保存一致受控备份后原子切换，没有强制结束在途调用。业务/schema/runtime配置hash保持，全部新Web资源实际服务hash核对通过。运行server SHA256为`824f73cb1cb4f5045e1bb45d5e1e4a8abf2fcb1ed9420d7670ebde6ef3253468`。没有用旧数据库覆盖新账务。

真实Chrome已登录会话reload后，总览按渠道正确显示`GPU03 CLIP`和`GPU03 FACE`，原“已删 #51/#50”消失，登录保持。缺名使用中性提示。原始截图与请求telemetry仅保留内部证据，不发布到仓库。

升级前后同口径6项合成调用均通过，全部HTTP200并核对attempt200：人脸正向检测1张/512维、CLIP文本和图片各512维、OCR合成marker、bge-m3-fallback1024维、本地qwen3.8-27b-gsq-iq3s Chat非空输出且finish_reason=stop。四透传及向量的响应hash与基线一致；Chat文本动态生成，不要求hash相等。此范围不包含原厂付费模型、工具、多轮或全部协议。另观察到BGE与GPT自然调用继续成功；不将HTTP状态未记录的自然调用标成HTTP200。透传成功率的既有统计口径未改，不宣称本次解决其0%显示。

## GitHub公开发行与独立验收

[发行工作流41637262505](https://github.com/qiwolf/tokengo/actions/runs/41637262505)成功，[正式Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.26)于2026-11-03T12:49:41Z（北京时间20:49:41）公开9资产。标签固定在c939，没有移动或重发。

9资产匿名下载、GitHub digest及包内校验/manifest、版本文档身份核对通过。两个变化的大包由Mac从官方对象URL以8段HTTP206并发下载，严格核对Content-Range和整包digest后同步build-test；其余7资产在build-test匿名下载，运行态验收均在build-test执行。初次慢速单连接下载的中止及切换收据保留。公开native包SHA256为`354e086f53448bcabccffe3ac77fecda9bea3f01e1dfb8364f76c924d4c5aea8`；公开镜像`ghcr.io/qiwolf/tokengo@sha256:a023141544f94c7f49a5fffe30d0c37985e3915f70396696562993c3e6965f6a`已在build-test实际匿名按digest拉取。原生/容器及Key生命周期、fresh schema42、无.env的Compose、nonroot/只读、重启/同卷重建与临时资源清理均通过。全部日志和公开产物已取回Mac。

公开WASM只核对静态身份与中性缺名标记，没有重跑公开包浏览器行为；真实Chrome名称验收针对237上部署的build-test配套包。公开CI包与237本地包来自同一源码但构建环境不同，各自字节SHA分别记录，不能当作同一个文件。新原厂兼容矩阵未重测；既有模型范围沿用[持续核实清单](../../provider-verification-checklist.md)原版本证据。

标签及包内文档保留构建时快照，发布后验收更新维护在main与Release说明，不回写或替换已发布资产。
