# qiwolf.18 公开发行收据

状态：**通过并已公开发布**。发行源码[`1b672c8d34cab98003ad4e6b45d9de4a4bee0e45`](https://github.com/qiwolf/tokengo/commit/1b672c8d34cab98003ad4e6b45d9de4a4bee0e45)；[v0.34.0-qiwolf.18](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.18)。Cargo0.34.0/schema41，linux/amd64。[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40181428585)和[发布流程](https://github.com/qiwolf/tokengo/actions/runs/40182556647)成功。

## 构建与测试

build-test完整工作区Rust **1345通过、0失败、3忽略**，Python分别python-container 3项、python-deploy 6项、python-mineru 13项。此前实现基线定向687项与这些完整检查分别保留，不相加为唯一总数。四条真实device开始/轮询pending/取消已验证，但没有真实用户登录、令牌刷新或真实上游推理验收。

## 公开资产

不可变镜像：`ghcr.io/qiwolf/tokengo@sha256:0d51c6733ca40cc08a297944e6d498723f1990e9734d850a50c80f6e0c4aa766`。匿名下载、匿名GHCR拉取、离线双标签、固定SHA无.env Compose、配套原生/容器生命周期及Key检查通过；临时运行状态清理通过，未访问生产。

| 文件 | 字节数 | SHA256 |
| --- | ---: | --- |
| [compose.yaml](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/compose.yaml) | 797 | `2c718f4b4b87629920fbda2ef322b14fc67877cff3067f71af958d1c6c260e80` |
| [container-deployment.md](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/container-deployment.md) | 15317 | `0b244bfccf77a36ef7a5160b9386dc7909dfd81e21905d3e06929ed5bb094ff7` |
| [container.env.example](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/container.env.example) | 316 | `5719f373c3e78bf5b75a9eac193af05b9e96c1c9dff18218d54e320d3c0caed4` |
| [image-SHA256SUMS](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/image-SHA256SUMS) | 564 | `d4e153fc2e5603e15b0f637930f2d9c6f348014b5bfec8905bd70d86b597282a` |
| [image-manifest.json](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/image-manifest.json) | 199 | `fd2270070c990e9abba42f44181600c847fa40f8b9a25c7d99616f539ba701c5` |
| [registry-image.txt](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/registry-image.txt) | 95 | `302b0eb06b366dadd07cd754573849eb3a66147a581290d2500aec66086ce2dd` |
| [tokengo-1b672c8d34ca-linux-x86_64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/tokengo-1b672c8d34ca-linux-x86_64.tar.gz) | 20486932 | `e3cfbac6e45ceabfe7d379540dd37fa015ee4940e03a076c6e7fc928f1e4e0ff` |
| [tokengo-1b672c8d34ca-linux-x86_64.tar.gz.sha256](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/tokengo-1b672c8d34ca-linux-x86_64.tar.gz.sha256) | 107 | `7d77f7346235e839bacd657474ad0892f69849ca756555b457afadf487a878e6` |
| [tokengo-1b672c8d34cab98003ad4e6b45d9de4a4bee0e45-linux-amd64.tar.gz](https://github.com/qiwolf/tokengo/releases/download/v0.34.0-qiwolf.18/tokengo-1b672c8d34cab98003ad4e6b45d9de4a4bee0e45-linux-amd64.tar.gz) | 40406855 | `decdca9292407bc2c1d6ae3e7cd3a12231dd881f670354f03b69f97966fdf76f` |

## 数据库与剩余边界

已通过的数据库可分发审计保持不变：schema41；新库与现场SQL对象差异0，schema40→41夹具升级检查通过。真实生产账号、私有配置和数据库值不随发行包分发；已记录资料的审计不等于未记录历史操作的穷尽审计。账号订阅权益与真实模型调用仍须用户逐项验收。

[机器可读收据](receipt.json)。标签与资产文档为构建快照，后补收据维护在main和Release页面，不移动已公开标签。

## 私有实例公开包对齐

237 已切换到本版实际公开下载包，运行服务端摘要与包内 manifest 一致。schema41→41、60张表，原有业务数据及保护配置保留，入口已恢复；独立 HTTP 检查 readyz 200、24个 Web 文件摘要一致、未认证 OAuth 管理接口401。已保留冷备；入口恢复后不可用冷备直接覆盖新写入。真实账号授权与模型调用仍待用户实测。
