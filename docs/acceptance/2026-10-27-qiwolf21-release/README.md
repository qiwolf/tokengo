# qiwolf.21正式发行与独立验收

2026-10-27，**[v0.34.0-qiwolf.21](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.21)**已公开发布并完成独立验收。发行源码`153c9db127c337c4863d966e81d7f9bf4b7b6ead`；Cargo0.34.0/schema42。main后补本收据，不移动已发布标签，不重新打包既有附件。

## 精确提交与检查

[完整Rust/WASM CI](https://github.com/qiwolf/tokengo/actions/runs/40460512848)成功：Rust **1398通过、0失败、5忽略**，另Python MinerU合同13项通过。数量取自本次精确提交的CI汇总；忽略项不算执行通过。忽略项：`db::requests::tests::stats_60k_performance_and_legacy_oracle`；`db::scores::tests::totals_60k_performance_and_legacy_oracle`；`tests::signal_child`；`real_registry_fetch`；`real_cloud_to_gpu_embedding_and_rerank_fallback`。

Mac保留源码与Git；583个Git条目经受控Mac源码bundle同步到build-test并核对精确SHA。build-test独立完成Linux x86_64服务端/CLI及WASM正式构建、Python部署6项与容器3项、原生打包和包内校验；构建后工作树干净，日志与产物全部取回Mac。

此前私有修复的Web153项单元检查包含5项新增回归，实际HTTPS管理员页面14项检查另行记录；250个受检源文件与发行源码逐字节一致。153、5及14均不与本次完整CI重复累加。私有浏览器检查覆盖桌面1470像素视口，本轮未新增移动端交互验收。

[正式发行流程](https://github.com/qiwolf/tokengo/actions/runs/40460556556)成功。同一发行提交生成server、CLI、WASM及Linux AMD64镜像，发布前检查精确main CI和产物身份。

## 公开产物独立验收

9个公开附件在build-test匿名实际下载，逐项核对GitHub资产SHA256及包/镜像校验文件。原生包`tokengo-153c9db127c3-linux-x86_64.tar.gz`的SHA256：

`628fcf9a6132d7f81922bad6883cbd777eef116514cbd4b455d04c8ab058bc4f`

GHCR按不可变摘要匿名实际拉取：

`ghcr.io/qiwolf/tokengo@sha256:4ab1acc7eb7ae389d6e138e4397626ec265e9909f2f0173ce0b63369224486d0`

版本、SHA及latest标签指向同一摘要；离线镜像双标签与源码身份一致。无.env固定SHA Compose、非root、只读根、ready/WASM、重启/同卷重建、原生运行及原生/容器Key生命周期通过。新建库schema42/60表、历史覆盖索引及完整性检查通过。临时进程、容器、卷、合成数据库、主密钥及登录材料清理通过。本地独立构建与GitHub公开包分别标识，不混用archive或二进制哈希。

实际下载的公开原生包内WASM包含“失效的已选目标”“移除失效目标”“已启用，暂无可用连接”“查看无可用连接的模型”四个标记，manifest与发行SHA一致。这是包内UI身份检查，不冒充重新执行14项浏览器交互。

## 行为、配置与生产边界

模型页提供无可用连接的查看入口，区分全局停用。路由编辑器允许逐项从草稿移除失效目标；取消还原，空范围或残留失效目标继续拒绝保存。管理员必须明确选择、保存和读回所需来源；安装包不自动修改用户私有路由或扩大白名单，其他策略字段保留。详见[维护说明](../../maintenance/stale-route-targets-20261027.md)。

客户端断开仅作原因调查，本版未修改客户端或网关取消、超时、连接处理逻辑，也未修改AG/HS。独立方案提交`0f9a02e1c6db91943ae8b3170a224335e9e76369`保留在本地，未纳入本次发布。

生产此前只更新私有Web构建`8bb60d26336f672b5866a59ff29cfb0be52d7d29`并手动修正目标模型的允许来源，其他34条路由策略摘要一致，服务端未重启；该阶段未进行真实上游推理或计费调用。本次发行没有部署完整生产包。公开产物的隔离验收不表示现场已换成server/CLI/WASM同提交完整包，也不代表真实供应商推理已验收。

schema42不变，无新增迁移，历史保留与权限合同沿用qiwolf.20。升级前一致备份数据库、主密钥、配置和程序；回滚不能用旧整库冷备覆盖上线后的新写入，已显式修正的模型策略可保留。

完整私有证据位于Mac本项目review/release-qiwolf21-20261027；此前修复证据在review/stale-route-repair-20261027。公开文档不包含凭据、现场身份或机器专用部署步骤。
