# 请求渠道可见性（2026-11-03）

请求详情和最近调用现在同时展示实际渠道名称、渠道 ID 与实际模型，保留客户端请求入口，便于识别虚拟模型最终落点。运行日志在上游开始和请求结束时记录 request_id、入口 model、channel_id、channel_name、served_model、upstream_model；JSON、SSE、失败和取消共用同一完成记录路径。

recent/detail API 的 request 对象新增可空 channel_name，与已有 channel_id 一致。渠道名来自当前配置，重命名后界面显示新名称；删除后保留原渠道 ID 和模型，显示“名称未记录或已删除”。未调用上游的请求不虚构渠道，运行中尚未选中上游时显示等待。无数据库迁移。

served_model 继续表示选中的逻辑模型；真实上游别名在 attempts.upstream_model 和运行日志的 upstream_model 中，不改变已有字段语义。历史文件日志不会被回填，新增日志字段仅对升级后的调用生效。

本次在 Mac 编辑，构建、测试与浏览器验收使用 build-test 的独立副本。所有接口测试使用本地合成上游，浏览器使用真实新 WASM 与合成 API 数据；不使用生产密钥或患者资料，不执行生产发布。验证结果见 validation.json；完整执行日志、源码 SHA256 清单、浏览器截图和构建产物保存在项目外层 review/request-channel-20261103/build-test/。

首轮回归在旧服务端观察到 recent.channel_name 缺失而失败。修复后覆盖虚拟入口首源失败走备用源、逻辑模型与上游别名不同、JSON/SSE 的成功与全部失败、recent/detail 一致、删除渠道后的 ID 保留，以及日志不包含测试密钥或请求正文。首次 green 编译因同步过滤了仓库合成 PEM 测试夹具而中断；只放行该已跟踪测试夹具后重同步，未复制真实凭据。

最终结果：服务端相关测试 751 通过、2 项原有忽略；Web 176 通过；Trunk release WASM 构建退出码 0。新增专项在测试生命周期及全局日志捕获修正后，完整 shared_virtual_routes 28 项通过。1440/390 浏览器验收通过，8 张截图；页面及弹窗无水平溢出，零浏览器错误或失败请求。浏览器数据为合成样本，不是线上运行验收。
