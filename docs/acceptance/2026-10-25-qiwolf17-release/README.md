# qiwolf.17 公开发行验收收据

状态：**已发布，公开产物验收通过**。验收日期：2026-10-25（Asia/Shanghai）。

[v0.34.0-qiwolf.17](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.17)，精确源码 [`0aced3ddbdcc8da38cead83b553f863804605bbb`](https://github.com/qiwolf/tokengo/commit/0aced3ddbdcc8da38cead83b553f863804605bbb)，Cargo **0.34.0**、schema **40**。Linux x86_64原生包和linux/amd64容器中的server、CLI与WASM对应同一发行提交。

## 验证依据

- [精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40159567667)和[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40159639375)已对该精确提交成功完成。
- build-test合并源码完整工作区：Rust **1253通过、0失败、3忽略**；Python三组分别为MinerU 13、deploy 6、container 3项。没有把本地与CI、旧定向检查或重复运行相加。
- 9个公开附件的digest/size与原生包逐文件清单已核对；实际原生CLI/server/WASM、匿名registry镜像拉取、离线双标签、固定SHA Compose、非root/只读根、Key生命周期、重启及同卷重建验收通过。
- 公开产物验收没有访问或迁移生产实例。此前供应商私有实测是独立证据，不代替本次公开包身份，也不代表所有账号、模型或HS完整链路。

忽略项保留实际身份，不能统称为凭据缺失：

- `tests::signal_child`
- `real_registry_fetch`
- `real_cloud_to_gpu_embedding_and_rerank_fallback`

## 不可变产物身份

- 原生包：[tokengo-0aced3ddbdcc-linux-x86_64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/tokengo-0aced3ddbdcc-linux-x86_64.tar.gz)，SHA256 `f6e109e4104dcfc1a2c73b9a09ac14e47ea5aa5d9307e29b043816a4dce58fc6`。
- 离线镜像：[tokengo-0aced3ddbdcc8da38cead83b553f863804605bbb-linux-amd64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/tokengo-0aced3ddbdcc8da38cead83b553f863804605bbb-linux-amd64.tar.gz)，SHA256 `1b960f817262125d8545fc3d039d05cc9beb04b03e2477238be7b892f73027ed`。
- Registry镜像：`ghcr.io/qiwolf/tokengo@sha256:7bba3a312f80960a0d79d01eb6e8dd61c371210126b6c042955767b8c41d88af`；version、`sha-0aced3ddbdcc8da38cead83b553f863804605bbb`与验收时latest指向同一digest，latest后续可变。

|公开附件|字节数|SHA256|
|---|---:|---|
| [compose.yaml](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/compose.yaml) | 797 | `751b93b3b0eefad025edf7a188f221b7111f8d7a79c57407f00bbb0cea527c76` |
| [container-deployment.md](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/container-deployment.md) | 13548 | `50745c3f6a95a9c5f292dac2fa465bb119d9e4783aedd644434a13db05d9c204` |
| [container.env.example](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/container.env.example) | 316 | `5719f373c3e78bf5b75a9eac193af05b9e96c1c9dff18218d54e320d3c0caed4` |
| [image-SHA256SUMS](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/image-SHA256SUMS) | 564 | `8729ce77c870626e4df605e8a0c616221ffceee579639367594a83cf49247c91` |
| [image-manifest.json](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/image-manifest.json) | 199 | `13f5b52b0a5226b8097cb2002c42ec83776eea0866be35dbe2b35f773a8bbade` |
| [registry-image.txt](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/registry-image.txt) | 95 | `4d677fb9c4f1e2538d0a44904b540e041b2d05577a0f9a25b0616bf8460d70b5` |
| [tokengo-0aced3ddbdcc-linux-x86_64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/tokengo-0aced3ddbdcc-linux-x86_64.tar.gz) | 20131874 | `f6e109e4104dcfc1a2c73b9a09ac14e47ea5aa5d9307e29b043816a4dce58fc6` |
| [tokengo-0aced3ddbdcc-linux-x86_64.tar.gz.sha256](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/tokengo-0aced3ddbdcc-linux-x86_64.tar.gz.sha256) | 107 | `3df7af74d002834efc32360f43da9ecfd6c4e369521c04c3e20e12ead9ee80a2` |
| [tokengo-0aced3ddbdcc8da38cead83b553f863804605bbb-linux-amd64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.17/tokengo-0aced3ddbdcc8da38cead83b553f863804605bbb-linux-amd64.tar.gz) | 40101737 | `1b960f817262125d8545fc3d039d05cc9beb04b03e2477238be7b892f73027ed` |

## 来源与快照边界

[源代码](https://github.com/qiwolf/tokengo/commit/0aced3ddbdcc8da38cead83b553f863804605bbb)、[CI定义](https://github.com/qiwolf/tokengo/blob/0aced3ddbdcc8da38cead83b553f863804605bbb/.github/workflows/ci.yml)、[发行定义](https://github.com/qiwolf/tokengo/blob/0aced3ddbdcc8da38cead83b553f863804605bbb/.github/workflows/release-check.yml)、[版本说明](https://github.com/qiwolf/tokengo/blob/main/docs/releases/v0.34.0-qiwolf.17.md)。全部记录采用公开GitHub链接，不包含机器专用脚本、私有日志、凭据或现场地址。

标签和包内文档保留构建时快照。完整发布后收据维护在main与Release页面，不移动既有标签，也不声称原下载包已包含后补收据。升级和回滚应保留新增业务账单，不用旧整库覆盖后续写入。公开生命周期检查不代替所有来源、账号或客户端的推理关闭语义验收；具体none兼容范围与冲突规则见版本说明。
