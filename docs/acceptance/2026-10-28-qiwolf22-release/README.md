# qiwolf.22 发行与237升级收据

2026-10-28已完成公开发行、独立公开产物验收和237整包升级。发行源码固定为 `f85eca9d67801a970ba93312a553da602deacea3`，Cargo0.34.0/schema42不变；后续收据文档提交不改变产品代码。[结构化收据](evidence.json)保存版本、产物摘要、计数及边界。

## 发行与源码验证

- [公开版本](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.22)。[精确main CI](https://github.com/qiwolf/tokengo/actions/runs/40559710708)成功：87个唯一Rust harness，1560通过、0失败、9忽略；MinerU Python13项通过，WASM成功。
- [发行流程](https://github.com/qiwolf/tokengo/actions/runs/40561989772)成功：同提交Linux服务端/CLI/Web、归档及架构校验、容器生命周期/持久化、离线镜像与发布。
- Mac提交通过Git bundle同步build-test，逐项Git tree一致；在build-test独立完成原生/Release Web、部署工具测试、打包和整包校验，源码保持干净，日志及产物取回Mac。独立包摘要 `00ef0ab8ec95079381a5cb46a403c9ea49f03ec193d33707c444d860273bc748`；它与公开包分别留证，不声称不同主机/归档时间的字节必然相同。
- [第四批验收](../2026-10-28-provider-p1-batch4.md)：受影响服务端699项、Web171项，OpenAI SDK24和Anthropic SDK3对照，SF4个精确托管型号14次网关调用。浏览器分轮实际流程及最终审计通过，62个Web输入逐文件匹配发行提交；保留全部原始失败轮和归因。

## 公开产物独立验收

build-test从公开渠道匿名下载9资产，逐项GitHub digest、文件大小及包内校验和一致；匿名按不可变digest拉取GHCR，核对version/SHA/latest三tag与离线双tag。镜像为 `ghcr.io/qiwolf/tokengo@sha256:187190dec8664793d9235e1ecf10c1e8c0efbf3fdd90bcee3a1630d475661fc0`。

原生包/CLI/Web身份、新库schema42与60表、90项运维动作边界、Key恢复/轮换/撤销、容器非root/只读根、无.env启动、重启和同卷重建均通过。公开WASM含11项本版关键文本；该静态身份检查不冒充公开包浏览器交互，交互证据来自同源码受控Release包。临时数据库、主密钥、Cookie、密码、进程、容器、卷和测试alias已清理；公开资产与脱敏报告已取回Mac。

|公开资产|字节|SHA-256|
|---|---:|---|
|`compose.yaml`|797|`6a632cb945119ebd45867939e4a826b700d68b5526bc8885df9f0591f4fa73c3`|
|`container-deployment.md`|15347|`dbd2124b09001c4f09b06da2b0bc5782370dac906df093210625eda6f03e963d`|
|`container.env.example`|316|`5719f373c3e78bf5b75a9eac193af05b9e96c1c9dff18218d54e320d3c0caed4`|
|`image-manifest.json`|199|`d6341e3b4e445c92ee4fbf9b58efe224690e4d63562d9fcac7d2868ad4764179`|
|`image-SHA256SUMS`|564|`a3ffaeb30c5e93de44e2f49f2c2aba23124184dbc1fbf5f72dcc77989064c077`|
|`registry-image.txt`|95|`92e1722af71d23fcafac3cb04ddead137cb2adcd74f66e108ff515d4da77e7f9`|
|`tokengo-f85eca9d6780-linux-x86_64.tar.gz`|21733849|`729cf853570736b47a5f85b1a288f6e3d83e48eb56b4c9054723da2c6be43d22`|
|`tokengo-f85eca9d6780-linux-x86_64.tar.gz.sha256`|107|`f0eb56d50b5539b4c1122db1b5637c0d16e241973ba5ad4bd491723909a89941`|
|`tokengo-f85eca9d67801a970ba93312a553da602deacea3-linux-amd64.tar.gz`|40737219|`c99f492ac641965cf522acba9c5b96b5411c18b1b6e5673abec790a9fed8c6c0`|

## 237完整升级与真实公网验收

旧实例是此前私有修复组合，本次替换为上述公开包的完整服务端、CLI和全部Web，不拼接旧组件。候选在CT内先校验归档/组件摘要、CLI及不接触数据库的原生执行；停止服务后对完整state（含DB/WAL/SHM/outbox/master）、配置和unit做冷备，核对内容并落盘，随后原子切换current并启动。

冷备保留在实例 `/var/backups/tokengo/full-release-20261027T190320Z-bd8aef6a`。原程序与页面保留用于回退；失败回退仅切回程序/静态资源，**不以旧整库覆盖新账务或会话写入**。主密钥、环境、原unit、静态API/网关Key、路由和用户配置指纹均未改变；OAuth刷新/派生缓存独立观测，本轮没有发生OAuth变更或撤销。

运行exe与公开server摘要 `e97aacef58efa9f49ef73bd547476824d11c822f494000566d68dbaa98aaffd1` 一致，CLI摘要 `04cdd789ea03b8f59ce682100ae8fe2fc89e9e34d7926f7feee4aeeddf052246`；本地readyz/数据库正常，24个静态资源逐项回读一致。公网TLS验证、24资产回读和模型可见性通过；合成2+3问答分别经Chat与Responses返回预期整数5，完整终态及已知usage通过，各1次真实上游尝试/1条已结算账本，响应与请求/账本usage一致，0遗留预占、0重复账本。没有重放生产正文，没有改客户端、创建或替换Key。

第一次CT直连GitHub下载超时发生在切换之前，原服务保留；记录未删除。随后传送已匿名验收的相同公开包，仍按GitHub摘要 `729cf853570736b47a5f85b1a288f6e3d83e48eb56b4c9054723da2c6be43d22` 验证后完成升级，没有降低校验或换包。

## 未充分验收与失败边界

本轮官方DeepSeek的任意字面标识复述样本未全部通过。直接官方system样本曾正确返回，直接developer为422；相同developer经TokenGo正确返回，支持角色改写生效。多system及合并system的官方直连同样出现标识改写/解释，网关Responses也未精确复述；没有证据据此强制合并system或修改转换器。早期未采正文的两次失败不倒推具体格式原因；保留不匹配，普通问答通过不覆盖此项未知，模型和Responses功能均未停用。

SF实测不外推原厂账号、其他型号或全部控制/工具。SDK为固定版本合成loopback对照；没有覆盖所有官方账号及真实Ollama/vLLM部署。G21–G29/O01–O09是方案独立可选接入，仍未实现；没有假称新增执行器。本地预算估算不是供应方硬上界，实际权限、归属、明确转换开关及具体不可表示请求校验继续生效。

空间核查见[第一批记录](../2026-10-27-provider-p0.md)：主因是历史构建副本和缓存而非日志，因此未新增日志保留选项、未更改现有账目保留策略。本批又清理约7.1GB已完成任务的可重建测试程序，保留日志、源码和发行产物。

## 私有原始证据位置

Mac工作区仓库同级 `review/release-qiwolf22-20261027/` 保存CI、构建及公开验收日志/产物；`review/provider-p1-batch4-20261027/` 保存SDK/SF/浏览器及 `deploy/` 升级、失败对照和公网调用收据。真实数据库、密钥、Cookie、密码均未纳入Git或公开产物；冷备只留受控实例。
