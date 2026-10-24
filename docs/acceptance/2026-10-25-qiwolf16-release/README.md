# qiwolf.16 公开发行验收收据

状态：**已发布，公开产物验收通过**。验收日期：2026-10-25（Asia/Shanghai）。

[v0.34.0-qiwolf.16](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.16)，精确源码 [`2702757fe4728a814fe135f0c102155c908abfae`](https://github.com/qiwolf/tokengo/commit/2702757fe4728a814fe135f0c102155c908abfae)，Cargo **0.34.0**、schema **40**。Linux x86_64原生包和linux/amd64容器中的server、CLI与WASM对应同一发行提交。

## 验证依据

- [精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40122964321)和[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40124162546)已对该精确提交成功完成。
- build-test合并源码完整工作区：Rust **1239通过、0失败、3忽略**；Python三组分别为MinerU 13、deploy 6、container 3项。没有把本地与CI、旧定向检查或重复运行相加。
- 9个公开附件的digest/size与原生包逐文件清单已核对；实际原生CLI/server/WASM、匿名registry镜像拉取、离线双标签、固定SHA Compose、非root/只读根、Key生命周期、重启及同卷重建验收通过。
- 公开产物验收没有访问或迁移生产实例。此前供应商私有实测是独立证据，不代替本次公开包身份，也不代表所有账号、模型或HS完整链路。

忽略项保留实际身份，不能统称为凭据缺失：

- `tests::signal_child`
- `real_registry_fetch`
- `real_cloud_to_gpu_embedding_and_rerank_fallback`

## 不可变产物身份

- 原生包：[tokengo-2702757fe472-linux-x86_64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/tokengo-2702757fe472-linux-x86_64.tar.gz)，SHA256 `234ffb528449541673a39e6a9f9d2a7502fbeedf9f026ea5085451608c8b9289`。
- 离线镜像：[tokengo-2702757fe4728a814fe135f0c102155c908abfae-linux-amd64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/tokengo-2702757fe4728a814fe135f0c102155c908abfae-linux-amd64.tar.gz)，SHA256 `95363ef1452074c45a26d8f1b2be39c9b068ed472bb5cebf168d16ba4d8e67cb`。
- Registry镜像：`ghcr.io/qiwolf/tokengo@sha256:3fff02def303b61ee2623ca856eacd2fd42aca13c8b2a00b278c383c610d18ca`；version、`sha-2702757fe4728a814fe135f0c102155c908abfae`与验收时latest指向同一digest，latest后续可变。

|公开附件|字节数|SHA256|
|---|---:|---|
| [compose.yaml](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/compose.yaml) | 797 | `e69259eaec7dd8a7b173d510bdb2a361f2bf2f9297dcd0ac75cd4ca96d9a17da` |
| [container-deployment.md](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/container-deployment.md) | 13548 | `50745c3f6a95a9c5f292dac2fa465bb119d9e4783aedd644434a13db05d9c204` |
| [container.env.example](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/container.env.example) | 316 | `5719f373c3e78bf5b75a9eac193af05b9e96c1c9dff18218d54e320d3c0caed4` |
| [image-SHA256SUMS](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/image-SHA256SUMS) | 564 | `1e0deb3c17a77978647ce0fd8960b37298f2a8f1c3db7ab70363b8baf2a6eeec` |
| [image-manifest.json](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/image-manifest.json) | 199 | `3a7ac343d3a6f592de9ff9e89f7777c251033949c5bef4b90987b0a649709d7c` |
| [registry-image.txt](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/registry-image.txt) | 95 | `d028fa26cd23c4352423fb28a953601672fba2f63c0da03eaf951d439dd1ab17` |
| [tokengo-2702757fe472-linux-x86_64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/tokengo-2702757fe472-linux-x86_64.tar.gz) | 20115708 | `234ffb528449541673a39e6a9f9d2a7502fbeedf9f026ea5085451608c8b9289` |
| [tokengo-2702757fe472-linux-x86_64.tar.gz.sha256](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/tokengo-2702757fe472-linux-x86_64.tar.gz.sha256) | 107 | `06c4b4cef17eb4f81ea73f28103e5e40dcf1134d5427ec637992e23b8b555d5b` |
| [tokengo-2702757fe4728a814fe135f0c102155c908abfae-linux-amd64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.16/tokengo-2702757fe4728a814fe135f0c102155c908abfae-linux-amd64.tar.gz) | 40101616 | `95363ef1452074c45a26d8f1b2be39c9b068ed472bb5cebf168d16ba4d8e67cb` |

## 来源与快照边界

[源代码](https://github.com/qiwolf/tokengo/commit/2702757fe4728a814fe135f0c102155c908abfae)、[CI定义](https://github.com/qiwolf/tokengo/blob/2702757fe4728a814fe135f0c102155c908abfae/.github/workflows/ci.yml)、[发行定义](https://github.com/qiwolf/tokengo/blob/2702757fe4728a814fe135f0c102155c908abfae/.github/workflows/release-check.yml)、[版本说明](https://github.com/qiwolf/tokengo/blob/main/docs/releases/v0.34.0-qiwolf.16.md)。全部记录采用公开GitHub链接，不包含机器专用脚本、私有日志、凭据或现场地址。

标签和包内文档保留构建时快照。完整发布后收据维护在main与Release页面，不移动既有标签，也不声称原下载包已包含后补收据。升级和回滚应保留新增业务账单，不用旧整库覆盖后续写入。本次不宣称HS全链路或等长32k/64k输出压力已验收。
