# qiwolf.27 发行与生产升级收据

2026-11-03（北京时间）。**已配套升级生产程序与面板，真实UI/现有调用验收通过；其后推送正式发行，公开产物独立验收通过。** 发行源码`497ec366d1c7d83b81858c22c1f1c4b982c23d54`，功能提交`ad434c7356a77c7af8b24661e6df46a091dba1d7`；Cargo0.34.0/schema42不变。安全结构化摘要见[evidence.json](evidence.json)。

## 构建与回归

Mac编辑源码/Git，build-test隔离副本逐轮核对输入。2项真实API合同先以字段缺失断言失败，修复后28项路由集成、178项Web单元通过，本轮全新优化WASM退出0。原生release193.19秒、打包4.25秒、manage.py verify0.75秒均退出0。最终含版本文档的722文件源码清单与包内manifest一致，28个本轮新Web文件hash逐一匹配，全部日志和产物已回Mac。

部署使用本地包SHA256`fe99db9b89f8ed3a88975b78f3f09d7ee748912cbc37faa53ac8414f182b2082`，源码清单SHA256`4ff881495659fe1207c9a4c0849a2a84b0eec88160fbeb7b65747a48aad76861`。该包与后续GitHub公开CI包源码相同、构建环境不同，分别记录字节SHA，不将它们视为同一个文件。

[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/41644499984)成功：完整Rust workspace **1584通过、0失败、9忽略**、MinerU Python13与WASM check通过。全量是在GitHub执行，不冒称build-test另跑全套；不与206项定向结果重复累加。

## 生产验收与发布顺序

先按零活跃/待结算/预留排空并保存一致受控备份，配套切换程序/Web。实际程序与全部新Web资源hash核对、业务/schema/runtime配置保全通过，没有强制终止在途调用。运行server SHA256`a4f31a9d26ac09e573caf6f10c59992ec07d1fd38cf670eec0ec1a97869d528a`。

真实已登录浏览器中，普通连接正确显示“供应商 · Key备注”，透传服务保留服务名，连接编号与分组说明可见；按模型仍显示独立模型维度。真实Key备注、账本ID、内部拓扑、截图及原始生产telemetry不进入公开仓库。

升级前后同口径6项合成调用均通过，HTTP200与attempt200：人脸正向检测/512维、CLIP文本和图片各512维、OCR合成marker、BGE1024维、原有本地Qwen Chat非空且finish_reason=stop。四透传及向量的响应hash与基线一致，动态聊天文本不要求hash相同；没有重试。此范围不外推原厂付费模型、全部型号、工具、多轮或其他协议。

本次仍按接入连接ID分组，同Key多连接不合并；Key备注就是用户原备注，可能本身含模型名，不自动篡改。原始连接名、Key备注、日志/模型归因、路由/auth保持；透传既有成功率口径和历史0%计数未在此次修复。生产部署、真实UI与6项后测完成后，才推送main/tag并触发正式发行，标签固定在上述源码。

## 正式发行与公开产物

[发行工作流](https://github.com/qiwolf/tokengo/actions/runs/41644506211)成功，[正式Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.27)于北京时间`2026-11-03 21:39:55`（UTC `2026-11-03T13:39:55Z`）公开9资产。9资产匿名下载、GitHub digest、包内checksum/manifest及精确tag文档身份核对通过。两个变化的大包由Mac从官方对象URL分8段HTTP206下载，严格核对Content-Range和整包digest后同步build-test；其余7资产在build-test匿名下载，运行态验收均在build-test执行。公开native包SHA256`871a22c28e887da91cbac68dc79d0d684516a96804d4b9c28228a82192b0191e`；镜像`ghcr.io/qiwolf/tokengo@sha256:e95b48fd7e9d24bf8667766737216996e2584099e5038073e638a84b3afaaf0e`已在build-test实际匿名按digest拉取。

原生/容器及Key生命周期、fresh schema42、无.env Compose、nonroot/只读、重启/同卷重建和临时资源清理独立验收通过，日志与9公开资产均回Mac。公开WASM仅静态身份/新说明标记检查，没有重跑公开包浏览器；真实UI验收针对已经部署的build-test配套包。

既有模型能力沿用[持续核实清单](../../provider-verification-checklist.md)注明的原日期/版本和范围。标签及包内文档保留构建时快照，最终收据更新在main与Release页面，不移动标签或替换已发布资产。
