# qiwolf.25 发行收据（2026-11-03）

发行源码 `3368cd6a29d87b76ec0d206f57be5e3440e952d4`。[精确 main CI](https://github.com/qiwolf/tokengo/actions/runs/41574671111) 与[发行工作流](https://github.com/qiwolf/tokengo/actions/runs/41574711093)成功，[正式 Release](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.25)于2026-11-03T04:03:28Z（北京时间12:03:28）公开9个资产。公开资产独立验收通过。

## 源码与执行环境

Mac保留源码、Git与文档编辑。build-test独立副本核对发行提交的707个跟踪文件SHA256，全数一致，仅仓库内test-only合成OAuth RSA夹具随测试源码同步。构建缓存复制到本任务独立目录，不把旧缓存当作源码验证。

本轮build-test完整Rust workspace **1579通过、0失败、9忽略**；Python MinerU/部署/容器分别 **13/6/3** 通过。精确GitHub CI另外得到同样Rust1579/0/9及WASM check成功，不与本地或先前定向751服务端/176 Web结果相加。9项忽略保留原测试的性能、helper、外部联网或显式验收边界。

本地原生release工作区、WASM release、打包与包内校验全部通过，8个检查步骤均退出0；构建前707文件SHA256一致，完成后源码保持干净。完整命令、退出码、时长、日志及产物身份见[结构化收据](receipt.json)，全部日志及本地产物已取回Mac。结果位于Mac项目根 `review/release-qiwolf25-20261103/`，不回传依赖缓存。

## 公开产物与独立运行

9资产匿名下载，逐一核对大小、GitHub digest、原生及镜像checksum、包内每文件校验、源码manifest和目标架构。主代理在Mac对已回收的9资产另行重算SHA256。

原生包SHA256：`13edc20146aa73dc6cb196932feda57b561792d49d9376c0f7f524bec1507606`。不可变镜像：`ghcr.io/qiwolf/tokengo@sha256:5f1ba50bcfcbd3586fa1b08f29f4d0a705b2b9f3ecec177c5c6edce27ad96d41`。离线镜像SHA256：`df274d8177000a904860dfea66847a3758c5496906ecf17bd0a6d7dd98fad9b3`。公开构建身份与本地构建分别记录，不声称字节相同。

GHCR匿名version/sha/latest标签在验收时指向同一摘要，并实际按digest拉取。公开离线镜像双标签、固定SHA Compose无.env启动、非root、只读根、ready/WASM、重启及同卷重建通过。公开原生与容器的Key生命周期、权限和fresh schema42/defaults检查通过。临时容器、卷、进程与凭据清理完成，既有资源保留。

详见[结构化验证](verification-q25.json)、[原生运行](native-runtime-q25.json)、[容器Key](container-key-runtime-q25.json)、[最终审计](independent-final-audit.json)。全部公开文件、执行脚本、命令退出码、日志、哈希清单已取回Mac上述目录的 `public/`，不含生产凭据。

6份包内文档与发行Git对象逐字节一致，见[文档身份](release-document-identity.json)。公开WASM具有实际渠道、删除渠道提示和服务商预设等新标记，见[静态UI身份](public-ui-identity-q25.json)；该项没有执行浏览器交互。此前1440/390合成API浏览器行为证据沿用前端/共享目录完全无差异的交付提交d631011，见[源码一致性](ui-source-reuse.json)和[渠道交付验证](../../maintenance/request-channel-20261103/README.md)。这不等于线上运行验收。

## 功能及生产边界

本版累计供应方预设、OpenCode目录及新建Go凭据默认会话头、请求渠道展示和运行日志归因。Cargo0.34.0/schema42不变，无新增数据库迁移或原厂真实模型验收。渠道名来自当前配置；删除后保留ID，历史文件日志不回填。范围见[版本说明](../../releases/v0.34.0-qiwolf.25.md)。

没有访问或升级生产实例，没有用真实厂商Key新增付费模型请求。OpenCode此前月额度HTTP429等历史限制仍保留。公开发布与生产整包升级分别验收。

标签和包内文档保留发布构建快照，后补收据维护于main与Release正文；不移动标签、不替换公开资产。
