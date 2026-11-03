# qiwolf.25 发行收据（2026-11-03）

状态：发布准备中，尚未取得正式发行及公开产物验收回执。本页在发布后补充精确源码、CI、工作流与产物身份；不移动已发布标签。

本版累计供应方预设、OpenCode目录及新建Go凭据默认会话头、请求渠道展示和运行日志归因。Cargo0.34.0/schema42不变，无新增模型原厂实测。改动范围见[版本说明](../../releases/v0.34.0-qiwolf.25.md)。

发布前定向证据：build-test服务端相关751项通过、2项原有忽略，Web176通过，WASM release构建通过；真实loopback虚拟入口首源失败走备用，JSON/SSE成功/失败、渠道删除、recent/detail与运行日志归因通过。1440/390浏览器在真实新WASM与合成API数据下通过。供应方预设的238项历史阶段验证与浏览器证据独立保留，不叠加重复数量。见[渠道交付记录](../../maintenance/request-channel-20261103/README.md)与[预设交付记录](../../maintenance/provider-presets-20261102/README.md)。

本轮新增完整工作区、Python部署与容器检查、原生/WASM打包和公开资产验收正在执行。执行主机build-test，Mac保留Git与源码；所有源码按文件哈希核验。仅测试专用合成OAuth RSA fixture可随测试源同步；不复制真实凭据。结果回收于Mac项目根的 review/release-qiwolf25-20261103/。

生产实例未升级；没有用生产Key新增付费模型调用。渠道名称查询当前配置，不是历史快照；已删除渠道保留ID；历史文件日志不回填，新增字段仅影响升级后的日志。
