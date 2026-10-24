# 共享DeepSeek真实调用及账务验收

2026-10-24（北京时间），在237已部署的模型修复 `be166df` 上，通过真实共享虚拟模型调用新增 SiliconFlow `DeepSeek-V4.1-Flash`。使用现有 credential6/source2/provider2，不复制或修改上游凭据，不更改普通模型路由。一次短非流式请求返回 HTTP200，内容 `DEEPSEEK_SHARED_OK`，输入23/输出8/总31 Token，upstream为 `deepseek-ai/DeepSeek-V4.1-Flash`。

请求15582、尝试记录及账本15033明确关联同一临时user54/key117和shared9。响应、请求及账本用量完全一致；source snapshot记录shared scope、provider2、credential6、source2及实际上游名，排除误走原共享fallback的情况。

## 账务口径

用户明确选择保留未定价。此来源/模型没有Token或按次价格，故本轮不验证金额扣费，也没有添加测试价格。账本status与usage_certainty均为unpriced，outcome为success，price_id/price_snapshot/debit_micro/observed_cost_micro均为null；实际用量为23/8，settled_ms非空，无未完成预占。界面显示“成功 · 未定价”，扣款和上游成本为“未知”；预占$0只表示当前没有占用，不表示这次上游调用免费。

## 清理及历史

临时共享入口只含一个SiliconFlow目标，验完通过正常回收站软删，原有3个共享入口保留。临时用户停用并归档；临时Key撤销、从有效配置列表删除，底层身份记录保留以便查账。未创建临时面板登录账号，未变更任何管理员密码。受控执行时使用的短时本地管理会话仅在目标进程内存中持有，结束即删除；没有把会话、网关Key或上游凭据写入证据。

本次真实请求和账本完整保留。对照执行前一致性备份，15581条既有终态请求、14846条尝试、15032条已结算账本、29704条分摊、14条Token价格、16条按次价格逐行核对，缺失或变更均为0。测试配置无活动残留；历史身份在账务查询里显示为关联历史对象。

备份保存在237的受保护目录 `/var/backups/tokengo/shared-deepseek-live-20261024T111559Z/before.db`，不复制到公开仓库。清理只通过身份与共享模型生命周期接口，不回滚数据库、不删除历史流水。

## 证据

- [实际返回与逐层对账](evidence/live-call-receipt.json)
- [清理及原有历史逐行核对](evidence/final-cleanup-history.json)
- [账务页面](evidence/ledger-filtered-history.png)
- [详情页面](evidence/ledger-detail-ui.png)
- [共享入口清理后](evidence/shared-active-after-cleanup.png)

本轮没有应用源码变化；build-test上的隔离回归与此前237部署记录另见同级维护目录。随后更新文档并按正式流程推送发布，发行提交与公开产物验证在发行记录中单独给出；不会把私有237构建的hash冒充公开流水线产物。
