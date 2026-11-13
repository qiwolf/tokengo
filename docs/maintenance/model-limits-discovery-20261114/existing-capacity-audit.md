# 现有计价容量配置审计（2026-11-14）

本轮审计17条当前CNY token计价配置，涉及15个来源。另看到1条按秒计价及12条按次价格记录；没有把后两类当作token容量配置。本审计仅查询目录、分析已保存快照，未修改生产。

## 已确定结果

来源56/60已由主任务按同Key目录声明修正，并有读回与真实调用收据：56为1048576/393216，60为1000000/384000。来源60在保留客户端128000输出预留及high推理参数后，短请求和约600KB长请求均实际命中60成功。目录里的context不能被解释成输入和输出可同时达到各自最大值；本轮也没有做极限饱和验收。

source64第一次目录读取超时。本轮从build-test使用既有受保护凭据管道只重试该来源一次，HTTP200、耗时2742ms，gpt-6-luna、gpt-6.1-terra、gpt-6.1-sol均匹配，但没有声明输入/输出上限。当前272000/272000不是从本次目录核实出的供应方容量，应保留未知状态，不能猜值。

|来源|模型/用途|当前输入/输出计价上限|证据状态|
|---|---|---:|---|
|1|qwen3.8 27b gsq iq3s|128000 / 128000|catalog_limits_unknown|
|2|bge m3|128000 / 0|catalog_limits_unknown|
|3|bge reranker v2 m3|128000 / 0|catalog_endpoint_404|
|9|bge m3|128000 / 0|catalog_limits_unknown|
|11|bge reranker v2 m3|128000 / 0|catalog_limits_unknown|
|27|gpt 6.1 terra|500000 / 128000|local_oauth_price_mismatch|
|28|gpt 6.1 sol|500000 / 128000|local_oauth_price_mismatch|
|31|gpt 6 luna|272000 / 128000|local_oauth_price_mismatch|
|54|bge m3|128000 / 0|catalog_limits_unknown|
|55|bge reranker v2 m3|128000 / 0|catalog_endpoint_404|
|56|deepseek v4.1 flash|1048576 / 393216|already_aligned|
|58|commandgo deepseek v4.1 flash|1000000 / 16000|partial_input_declaration|
|59|commandgo deepseek v4.1 flash|1000000 / 16000|partial_input_declaration|
|60|opencode deepseek v4.1 flash|1000000 / 384000|already_aligned|
|64|gpt 6 luna|272000 / 272000|catalog_limits_unknown|
|64|gpt 6.1 sol|272000 / 272000|catalog_limits_unknown|
|64|gpt 6.1 terra|272000 / 272000|catalog_limits_unknown|

来源58/59目录只给出了1000000输入/context声明，与当前输入cap数值相同；未声明输出cap，16000不能认定为真实供应方最大输出。

来源27/28的价格输入cap500000、来源31为272000，均小于本地OAuth绑定872000；这能确认本地配置不一致，但OAuth绑定本身不是供应方证据，不足以直接把三个模型都扩成872000。

6条向量/重排配置（2/3/9/11/54/55）的128000是请求聚合预占上限，不能等同于单条文本、单个query-document对的模型context。多文本batch、重排多文档会累积用量；不能用单项context替换batch总上限。输出为0对非生成端点本身并不异常。来源3/55的GET /models为404，只说明该目录端点不可用，不说明重排服务或模型不存在。

来源1的目录可返回并匹配模型，但未声明窗口。其余模型也不能仅靠名字、已有价格、启用状态或小请求成功来推定最大容量。

## 可确定改动与CAS方案

已完成且有证据的容量改动只有主任务的56/60。本审计未发现其余15条配置存在足够完整证据、可立即执行的容量修正；因此本审计生产写入列表为空。

可保存的证据缓存包括source/model、抓取时间、原始声明字段名与语义、目录hash、凭据身份引用（无实值）及declared/partial/unknown/error状态。source64记录“重试成功但未声明”，不再混写为超时。58/59只缓存输入声明；3/55保留endpoint404。缓存更新不暗中修改价格。

后续若取得新合同：立即重读价格最新版ID、来源mapping/base_url和协议JSON；以原ID及配置hash做CAS；只新增必要价格版本或按现役生命周期接口更新指定字段，保留币种、费率、归属、启停及历史。某行CAS冲突只暂停该行。修改后读回并记录回滚引用。向量batch与单项窗口分字段处理。完整可机器读取方案见existing-capacity-audit.json。

## 证据与边界

证据均在本目录：existing-inventory.json（17条配置快照）、existing-catalogs.json（初次目录）、existing-catalog-source64-retry.json（定向重试）、model-catalog.json/official-catalog.json（56/60同Key声明）、alignment-result.json/alignment-readback.json（主任务修正与收据）。完整hash在JSON。

本轮未重新读取所有生产配置，审计时点以existing-inventory快照为准；后续提交修改必须再次CAS核对。目录声明与真实极限验收分开，未知容量保持未知。

## OAuth历史来源补核

Sol/Luna的872000已有2026-10-20同账号精确目录及配置读回出处，并非随手设定。当前账号目录定向读取401，未复核；27现已改映射exact Terra，其容量不能沿用旧Sol目录。三价格均暂不猜改；状态blocked_by_current_catalog_evidence。停止进一步认证尝试，未刷新会话或写authDB。详见oauth-bound-provenance.md/json与oauth-direct-catalog.json。
