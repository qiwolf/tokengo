# Chatbox 系统提示兼容修复及237验收

已提交请求修复 `0ea696d` 和真实调用发现的响应兼容修复 `92f560a`，237实际运行后者。开头纯文本 system 提示转换为 instructions；非流式订阅响应的已观测配置回显经校验后保留到 provider_metadata.openai_responses。通用转换器和原生 Responses 边界保持不变。

build-test同步206文件并逐项核对SHA-256，71项相关测试通过，Linux后端release构建成功；日志、源码清单及二进制已取回Mac。仅替换后端，前端和CLI继承原qiwolf.10；本次为私有热修复，没有推送GitHub或发布新标签/容器。

|真实调用|HTTP|耗时|验收|
|---|---|---|---|
|gpt-6.1-sol，stream=false|200|2.50秒|完整JSON、stop，精确返回系统随机口令|
|gpt-6.1-sol，stream=true|200|3.66秒|SSE共13个JSON事件、stop和[DONE]，精确返回另一系统随机口令|

两次均通过237 nginx的 `/v1/chat/completions`，真实出站渠道28、上游模型gpt-6.1-sol；请求955/956及attempt、已settled账务均关联成功，usage_source=upstream。用户消息故意要求覆盖系统指令，两次均遵循系统口令。现有账务为unpriced，本次不调整价格或宣称价格验收。

切换前排空、整目录冷备，校验schema40/59表、业务授权表、env/master/unit保持一致，四个web资产回读通过。最终实际二进制哈希匹配build-test产物，服务/nginx/readyz正常；原版本及两轮冷备均保留，路径见receipt.json。

本机旧管理凭据及旧个人Key已失效。实际验收使用服务器上Andy现用Key，仅在服务器进程内解密使用，不导出明文、不创建/修改Key；原policy及全部94个既有Key逐行哈希检查不变。

本次最初非流式调用还暴露access_programs等新增响应字段被严格转换器拒绝，已取真实结构并补回归后修复；失败证据保留，不把它记为通过。完整证据及产物位于主项目 `review/chatbox-system-20261023/`，第二轮构建在 `evidence/round2/`、`artifacts/round2/`，生产回执在 `deploy237/evidence/`。

验收为真实上游API调用，未操作Chatbox客户端，也未验证外部TLS入口；Chatbox重试由用户完成。源码与环境没有变化时无需重复本轮测试。
