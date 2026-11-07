# 重排本地优先、繁忙时转云端

共享入口 `bge-reranker-v2-m3-fallback` 保持名称，ordered目标为provider1的单节点 `bge-reranker-v2-m3-gpu03`、provider1的 `bge-reranker-v2-m3-gpu01`、provider2的 `bge-reranker-v2-m3`。两个本地alias各只有一个真实来源，设置既有local round_robin以忙时跳过；单节点轮转偏移恒0，实际优先级由共享目标顺序决定。不能靠channel_ids推断共享目标单节点性，必须核对实际映射。

本地两个source与credential并发上限各1，跨逻辑alias共享占用。空闲用GPU03；其有任务则用GPU01；两者有任务则立即尝试云端。网关只认识经它接收的在途重排，不据GPU利用率推断Whisper等其他任务，也不能追踪绕过网关的直连请求。

修复繁忙本地之后的云端请求失败还回头排本地的异常分支。云端优先、先失败再遇忙本地的原有等待行为保留。未改鉴权、计费、输入窗口、完整候选或调用方超时；云端失败仍返回实际错误，并不承诺三秒内完成。

build-test验证：旧实现复现失败；修复后server库及local_balance、shared/user virtual、vector_fallback、rerank_compat共685项通过，4项忽略；Linux release构建通过。三项配置事务测试覆盖单节点身份、保留新业务账的回滚、拒绝覆盖后续配置。

本次为本地运维补丁，没有推送或发布新版本。生产切换和真实请求来源结果以[本地交付证据](../../../../evidence/rerank-priority-20261107/RESULT.md)为准。
