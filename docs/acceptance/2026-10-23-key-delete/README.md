# 已撤销 API Key 删除能力验收

已在237部署：撤销的Key列表项提供“删除 Key”，独立确认窗口说明影响，支持取消；活动/暂停Key不能删除，须先永久撤销。普通用户仅能删除本人已撤销Key，管理员可管理删除；POST删除幂等，原有DELETE撤销接口语义保持。

删除采用settings的deleted_key:ID标记，不删除凭据物理行、父子关系、用量账务或历史审计，不创建新schema。用户和管理员的identity列表均过滤删除项；删除项不能继续编辑/重新启用，已撤销父Key的子Key继续被拒绝。首次删除与key.delete审计在同一事务，审计失败会回滚标记。

新增接口：POST /api/account/keys/{id}/delete（本人浏览器会话+CSRF）；POST /api/vkeys/{id}/delete（管理授权）。成功{key,deleted:true,id}，未撤销/仅停用409，跨用户404，已删除编辑404。现有独立运维令牌的白名单未扩展，新路由默认拒绝。

build-test验证：用户管理17、身份/代理/运维令牌46、Web124，共187通过/0失败。覆盖权限、CSRF、幂等、父链及7micro历史扣费保留、审计失败回滚；这不是完整workspace重跑。服务端release与配套WASM构建通过，302文件源码摘要核对通过；base0ecd357加本轮补丁，精确来源及产物摘要见build-proof.json/web-proof.json。

237升级前一致冷备、业务配置保全；schema40/59表，旧release与旧hashed前端资源保留供回滚。运行中服务端与产物SHA一致，直连/nginx资产匹配。现场用andy浏览器验证：撤销Key有删除按钮，Hermes活动Key无按钮，取消保留、确认删除、刷新后不再出现均通过。仅清理上轮创建的临时83/84，物理行仍在、各有一条key.delete审计；Hermes82保持启用。

本项及上一轮明文交付修复位于qiwolf.8之后的main和237部署；未重写原qiwolf.8标签/公开二进制或容器。管理员流程及跨用户/失败场景通过隔离集成测试验证，未在生产root浏览器重复。

全部日志、产物、截图与执行脚本已取回主项目review/key-delete-20261023/；数据库最终integrity=ok、外键错误0，无未结算账务。浏览器验证不输出真实Key明文。
