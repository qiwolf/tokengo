# qiwolf.24 发行收据（2026-11-02）

发行源码 `589f02c6a60bdfab9affed67fda398dba3e092a4`。[精确 main CI](https://github.com/qiwolf/tokengo/actions/runs/41415141711) 与[发行工作流](https://github.com/qiwolf/tokengo/actions/runs/41416249807)成功，正式 Release 公开 9 个资产；build-test 公开资产匿名下载、GHCR 实际拉取及原生/容器独立验收通过。

## 源码、执行环境与构建

Mac 保留源码和 Git，独立构建副本位于 build-test `/srv/build/tokengo-release24-20261102/source`；685 个跟踪文件逐一核对 SHA256，包含本轮提交后的全部文档及测试，未同步真实凭据或本机依赖。仅仓库内标为 test-only 的合成 OAuth RSA fixture 随测试代码同步。

本轮 build-test 完整 Rust 工作区 1570 通过、0 失败、9 忽略；MinerU13、部署6、容器3项 Python 通过。原生 release 工作区、WASM release 构建、打包及包内文件校验全部通过。GitHub 对同提交另外执行 Rust1570通过、0失败、9忽略和 WASM check；不叠加重复覆盖数量。9项忽略保留各测试本来的性能、helper、外部联网或显式验收边界。

初次远端预检因同步副本的文件属主与执行账号不一致，被 Git 的目录属主检查拒绝，未进入项目测试。修正任务目录属主并重新同步、核对哈希后完成检查；原预检日志保留。构建缓存从此前已停用任务复制至本任务独立目录，验证使用本轮源码，不把缓存当成源码证据。

全部执行命令、目录、退出码、时长、日志与本地构建产物已取回 Mac 项目根目录的 `review/release-qiwolf24-20261102/`，其中 `build-test-evidence/` 为本轮检查，`local-artifacts/` 为本地构建，`public/evidence/` 与 `public/assets/` 为公开产物验收及下载资产。没有回传依赖缓存。结构化汇总见 [receipt.json](receipt.json)。

## 公开资产与独立运行

公开原生包 SHA256：`2f40fc9f1146eeb9cfeef124f53fac398bc69ac5cadc6eee438bb7c42966b103`。不可变镜像：`ghcr.io/qiwolf/tokengo@sha256:07ffb081be6402825488f03c20fad1600ae415d2d629f9edce0e3159f1f4c8f5`。二者是 GitHub 公开构建的身份，与 build-test 本地构建产物分别记录。

9 个正式资产匿名下载，并逐项核对大小、GitHub digest、包内 checksum 和精确源码 manifest。匿名 GHCR version/sha/latest 标签指向同一摘要，实际按 digest 拉取；离线镜像双标签、固定 SHA Compose 无 .env 启动、非 root、只读根、ready/WASM、重启及同卷重建通过。公开原生/容器的 Key 生命周期、权限与新库 schema42/defaults 等隔离检查通过，见 [verification-q24.json](verification-q24.json)、[原生运行](native-runtime-q24.json)、[容器 Key](container-key-runtime-q24.json)和[最终审计](independent-final-audit.json)。

公开包 WASM 静态 UTF-8 标记核验见 [public-ui-identity-q24.json](public-ui-identity-q24.json)，它不是本版浏览器交互复测。新增来源 IP 指南、版本说明和现场配置与发行提交逐字节一致，见 [source-ip-document-identity.json](source-ip-document-identity.json)；该静态身份检查不代替模型请求来源落库验收。临时原生进程、容器、卷、状态目录和任务镜像别名清理通过，既有容器/卷保留；公开文件留存本任务缓存。

## 范围与生产分界

本次为可信反代来源 IP 指南、237 配置实测记录与此前 CI 测试维护；Cargo0.34.0/schema42不变，应用业务逻辑、依赖和模型能力没有新增。既有来源、精确型号和认证范围继续引用注明版本的历史证据，不标为本轮真实厂商新测。

237 的 Nginx 配置已独立上线，公网健康请求在 VPS/237 的来源一致，伪造头与不可信直连检查通过，详见[现场记录](../../maintenance/source-ip-20261102/README.md)。本次未把新版本完整程序包升级到237，没有为来源检查额外生成付费模型推理，也未独立取得新增公网模型请求数据库记录。升级程序不会自动修改 Nginx。

标签和包内文档保留发布构建快照；本次后补收据维护于 main 和 Release 页面，不移动标签、不替换已发布资产。
