# 2026-11-14 计费预占与 fallback 专项补丁

修正长JSON字节上界被误当成实际token窗口超限、本地来源容量不足终止fallback的问题。预算预占、真实用量结算、上游400开关与原生会话绑定分别保留其语义；不修改现有价格或路由数据。Cargo0.34.0/schema47保持，未公开发行。

[实现、build-test与生产收据](../maintenance/billing-reservation-fallback-20261114/README.md)。
