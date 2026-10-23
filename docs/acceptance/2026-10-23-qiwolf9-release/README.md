# qiwolf.9公开发行验收

本次验收对象为已公开[v0.34.0-qiwolf.9](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.9)，发行源码`8598fc070ca4a91e700d5748947d0343766ad06b`，Cargo0.34.0、schema40。原生包Linux x86_64、容器linux/amd64；所有下载和运行均在build-test隔离目录完成，未访问或升级237。

## 源码与公开产物

精确提交[CI](https://github.com/qiwolf/tokengo/actions/runs/39898866368)与[发布流程](https://github.com/qiwolf/tokengo/actions/runs/39898890902)均成功。最终源码完整workspace为 **1103通过、0失败、3忽略**，另 **22项Python通过**；GitHub CI独立验证。历史214项定向检查和补丁重复运行不与这些数量叠加。

9项公开资产匿名下载校验通过，原生包SHA256为`8ef62e86b8f845add9d1404432980afb8776b7071ada89f39a740c11b26cea3a`，不可变镜像为`ghcr.io/qiwolf/tokengo@sha256:a25ef92a226b8921e3339d744e718572f10fd7bf7e7b4258e1e8c77d7aa5250a`。manifest/镜像源码与发行提交一致；GHCR匿名按摘要拉取、离线双标签、无.env固定SHA Compose实际启动、非root/只读根、重启和同卷重建通过。version、sha与latest在验收时指向同一摘要，latest随后可变。

## Key运行合同

公开原生包和公开镜像各启动独立临时实例，验证本人及管理员创建Key后精确再次显示，响应no-store，安全列表投影不含明文或密文；匿名、缺CSRF、普通Bearer和运维令牌不能绕过权限。撤销后可管理查看但不可调用，删除后两种身份都不能读取且列表隐藏。

原生隔离数据库另核对恢复材料以密文存储，删除保留物理Key与审计且审计不含明文，并创建只存摘要的历史合成项验证409明确无法恢复。容器验收是实际HTTP合同，不声称读取容器或生产数据库验证加密细节。所用凭据仅为隔离合成数据，不导入真实Key，不在本收据保存任何Key原文。

## 证据与边界

最小收据：[公开检查](public-verification.json)、[资产摘要](public-assets.json)、[镜像标签](registry.json)、[原生Key运行](native-runtime.json)、[容器Key运行](container-key-runtime.json)、[完整测试](tests.json)、[GitHub流程](github-workflows.json)。原始日志和大型产物保留在任务review目录，不打包进本目录。

这次公开产物验收未修改237，也不证明237运行二进制与公开资产相同。此前现场普通HTTP嵌套弹窗的系统剪贴板、关闭清除DOM和刷新再次读取见[阶段记录](../2026-10-23-key-reveal/README.md)；不能把本次API运行检查扩大为所有浏览器或客户端验收。历史Hermes无密文无法恢复的限制保留，本轮未更换原Key。

标签及公开资产内文档是发布前源码快照；本发布后收据保存在main与GitHub Release说明，不代表先前附件包含这份后补结果。完整说明见[版本记录](../../releases/v0.34.0-qiwolf.9.md)、[总体状态](../../acceptance-status.md)、[账号合同](../../account-login.md)。
