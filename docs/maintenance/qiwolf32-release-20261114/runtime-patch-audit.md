# qiwolf.32：运行补丁发布审计

2026-11-14。审计范围为六个已在部署产物中保留、尚未完整纳入Git的补丁文件。本轮发布将其纳入可复现源码；不把已有本地review目录或其原始运维数据批量提交。

## 结果与发布说明

- 空终态流诊断只记录有限个事件类型、输出项类型、终态和输出token数，便于定位上游200但无有效输出；不记录正文、工具参数、加密推理、响应ID、请求头或凭据。
- 来源容量调整不再改变来源身份代次；协议、转换、运行时身份等实际能力变化仍递增代次。修复损坏旧协议文档时也会递增，不把非法旧配置视为相同身份。
- 新管理界面保存来源协议时，同时提交读回的binding_generation和原始协议文档SHA256，防止容量编辑不改变身份代次后产生静默覆盖。服务端在同一事务内核对两者。旧客户端未发送协议hash时仍兼容原代次校验；不宣称其获得了新增hash保护。
- 已保存凭据可直接进入“参数与转发”，从已保存source_ids定位来源，无需重新保存Key或模型映射。页面说明协议与容量配置作用于所选来源及其全部模型。
- 本轮修正过时的“按用户Key粘性”界面文案，改为按当前Key策略、顺序与可用容量选择来源。协议配置注释同步说明：无TPM时不因未知费用估算拒绝opaque/audio，有显式TPM时才要求相应输入计量依据。未恢复会话锁源或本地金融门槛。

## 六文件范围与实现检查

|文件|作用与审计结果|
|---|---|
|`crates/server/src/gateway.rs`|PrecommitShape只接收静态白名单标签及整数；events/items各最多32项。错误日志附shape，不改变流式选源、正文透传或计费逻辑。已有隐私/长度回归。|
|`crates/server/src/protocol_config.rs`|revision对原始持久化协议JSON取SHA256；save比较经过验证的完整能力对象，只排除两个容量字段。非法旧配置不能进入“身份未变”分支。现有runtime_identity校验继续执行。|
|`crates/server/src/routes/admin.rs`|管理读取返回protocols_sha256；保存仍要求管理授权、来源存在、真实OAuth协议约束。可选hash与必需来源代次在事务内CAS；成功返回新的hash。|
|`crates/server/tests/channel_protocol_patch.rs`|覆盖容量更新保持身份代次、raw hash变化、旧表单409、非容量协议变更递增身份。没有以跳过测试掩盖冲突。|
|`crates/web/src/pages/connection_parameters.rs`|保存payload从实际读回行取得两份版本；缺失/非法hash先重新读取，不伪造hash或代次。仅修改选中来源。|
|`crates/web/src/pages/provider_keys.rs`|进入既有接入编辑时使用已保存source_ids，参数页导航不写凭据/模型；无来源时保留未就绪状态。新增导航回归不读取私密Key。|

未发现六补丁引入未授权来源、泄露凭据、数据库迁移、生产路由改写或新的会话绑定。对多来源凭据仍按页面选择单个来源，不把设置静默扩为同凭据的所有来源。

## 已部署与已测证据

六文件修改前的逐字节hash均与两次归档源码清单匹配：`no-local-budget-20261114/deployment-source.json`和`mobile-layout-20261114/deployment-source.json`。前者部署执行收据exit_code=0，目标server SHA256为`c23913c900196e7208d76674c039549d222a26ae46c3127d638ff7184a1ed2d1`；后者Web-only部署exit_code=0，运行PID、server、环境和配置保持不变。因此这些补丁主体属于已部署代码，不是凭“Git未提交”猜测其生产状态。

旧证据由build-test产生，保留在原工作目录review中，不自动公开原始运维记录：

|证据|精确范围与结果|
|---|---|
|`no-local-budget-20261114/evidence/release-release-tests.log`|gateway::precommit_shape_tests、protocol_config::capacity_identity_repair_tests和channel_protocol_patch的容量CAS回归均记录ok。该批次整体exit101，不能当作全套通过；失败的是随后修订的其他旧合同测试。|
|`no-local-budget-20261114/evidence/accepted-accept-tests.json/.log`|后续四个受影响integration target全部通过，执行主机build-test，exit0；不把此结果扩大成重新执行全部workspace。|
|`no-local-budget-20261114/evidence/green-web-tests.json/.log`|Web库223项通过，包含保存payload双版本/缺hash拒绝及已保存source导航回归。|
|`no-local-budget-20261114/evidence/source-limits-qa-green/browser-results.json`|来源参数页面两项浏览器场景通过；浏览器范围以该收据自身为准。|
|`mobile-layout-20261114/deployment.json`|Web-only生效，server/配置身份前后相同；只证明该次部署，不代替公开发行资产核验。|

本轮仅在provider_keys更正一处产品文案、在protocol_config更正两段陈旧注释，六补丁业务逻辑未追加改变。新hash仍须纳入本轮exact源码测试、WASM和包身份；不能将文案修订后的源码冒充旧部署同字节版本。

## 源码hash

表中“已部署”取两份相互匹配的部署清单；当前hash取本次审计完成时的Mac工作副本。提交后的commit应由发行主流程填写，本文不预造commit。

|文件|已部署源码SHA256|当前审计源码SHA256|状态|
|---|---|---|---|
|`crates/server/src/gateway.rs`|`b8b0a8eefb7eddc5af20c13fdd0f851b93c981ac371a2a4a8e8c21ba46baa136`|`b8b0a8eefb7eddc5af20c13fdd0f851b93c981ac371a2a4a8e8c21ba46baa136`|与已部署源码相同|
|`crates/server/src/protocol_config.rs`|`24f1b64ad288a1786f3f0f90923b48a4652df0fd544ec5c01e50bb6be3f5eccb`|`e36b9f5989751ed575976c2e8d3ee6f57ba2136978f597984327d51e1c45d0b7`|本轮仅修正文案/注释，需新源码验证|
|`crates/server/src/routes/admin.rs`|`d5588fe89074cbb7fdd869ac09741014cded050c48bf0fda53fe444c5745c90c`|`d5588fe89074cbb7fdd869ac09741014cded050c48bf0fda53fe444c5745c90c`|与已部署源码相同|
|`crates/server/tests/channel_protocol_patch.rs`|`8045af51bfa91d0f4498ee60a4843921303a4f210bf9b30cc4ed8143cbb9e032`|`8045af51bfa91d0f4498ee60a4843921303a4f210bf9b30cc4ed8143cbb9e032`|与已部署源码相同|
|`crates/web/src/pages/connection_parameters.rs`|`c8c8a51dcfd2ce86623e547bc6750e277f47e40a5aae38e10a3dc5c57dbe44ae`|`c8c8a51dcfd2ce86623e547bc6750e277f47e40a5aae38e10a3dc5c57dbe44ae`|与已部署源码相同|
|`crates/web/src/pages/provider_keys.rs`|`4f304544adcd37729498c207149db18ccf775676ad1594e48b6fbcf8d10589a2`|`ea93375ec3cb8e79f679c2a1422f00f0d57180fb4102a400f0ed7f4521bbef26`|本轮仅修正文案/注释，需新源码验证|

## 本轮验证命令与边界

源码编辑和Git管理留在Mac。先同步包含未提交文件及删除/重命名的最新exact源码到build-test独立目录、记录清单hash，远程开始先核对hostname。以下命令仅为root统一执行的清单，本审计没有在Mac运行测试或自行提交Git。

主流程若已对同一exact源码完成workspace测试，无需再次重复下面定向测试；只有新修改或失败才重跑受影响项。

```sh
# build-test；在本轮独立的Linux源码副本执行
cargo test --locked --offline --release -p tokengo-server --lib
cargo test --locked --offline --release -p tokengo-server --test channel_protocol_patch
cargo test --locked --offline --release -p tokengo-web --lib
# 必要时用过滤器单独定位六补丁：
cargo test --locked --offline --release -p tokengo-server --lib precommit_shape_tests
cargo test --locked --offline --release -p tokengo-server --lib capacity_identity_repair_tests
cargo test --locked --offline --release -p tokengo-web --lib save_payload_
cargo test --locked --offline --release -p tokengo-web --lib existing_parameter_navigation
# 本轮公开发行仍需实际Linux server/CLI与WASM构建，非仅测试代码：
cargo build --locked --offline --release -p tokengo-server -p tokengo-cli
(cd crates/web && trunk build --release)
```

浏览器定向检查：打开已保存Key→直接进入参数页→选择不同来源；核对没有重存Key/模型的写入；容量保存保持代次但更新hash；另一窗口旧hash保存返回409且不覆盖；协议更改递增代次；运行时身份字段仍保留。菜单/布局使用本轮真实bundle检查，不以源码字符串推断渲染通过。

公开包核验还须对server/CLI/Web文件、manifest/checksums、来源commit及发行附件进行读回比对。该步骤由root发行流程完成；此文不把本地包、已部署私有补丁或Git提交单独写成公开发布完成。
