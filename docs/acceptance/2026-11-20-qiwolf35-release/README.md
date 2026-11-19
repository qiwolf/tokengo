# v0.34.0-qiwolf.35 发行收据

状态为发行准备。GitHub CI、正式发布和公开制品验收结果将在对应步骤完成后补记。

程序版本为 `0.34.0-qiwolf.35`，数据库 schema 为 50。功能范围见[版本说明](../../releases/v0.34.0-qiwolf.35.md)。

- 用量折叠提交 `6a2a19b` 已在 237 通过实测。同一真实账目区间内，右侧 9 项默认显示 4 项，图表说明到卡片底边由 280px 缩至 17px。[生产收据](../2026-11-19-usage-model-collapse-deploy237/README.md)
- Key 模型候选筛选提交 `4172f82` 已通过 build-test 的 225 项前端单测、WASM 构建和 Chromium 合成账户回归，覆盖授权保存前后及新建和编辑页面。237 使用现有用户真实授权验证新建 Key 的候选从 31 个收至 18 个，未授权候选为 0。[生产收据](../2026-11-20-key-user-model-scope-deploy237/README.md)
