# 通用 HTTP 透传

透传路由将一个路径前缀连接到固定的 HTTP(S) 上游。适用于人脸检测、CLIP 和其他自定义 HTTP 接口；不会解析业务正文，也不会把这些接口登记为 OpenAI 模型。

本功能的配置由服务端 SQLite 管理，通过面板和管理 API 修改，不新增 YAML 配置源。路由配置中的上游 Header 值加密保存，读取接口只返回名称及是否存在值，面板不会回显明文。

本功能自qiwolf.6公开；当前发行版为[v0.34.0-qiwolf.7](https://github.com/qiwolf/tokengo/releases/tag/v0.34.0-qiwolf.7)，修复容器交付的固定镜像引用和离线导入标签，不改变透传业务合同。部署实例仍运行qiwolf.6源码 `d5660992a6a1ef0b320ff143a19404ac3abad23e`、schema39；本轮未升级该实例二进制，不能把qiwolf.7公开等同于现场软件升级。

2026-10-21的qiwolf.6发布验收使用临时路由，结束后已清理。2026-10-22另行完成正式接入：Face、CLIP/OCR和MinerU三条正式路由启用并保留，各有独立VKey，仅授权自己的路由及计费接入，设置 `allowed_models=[]`。当前每次网关价格为0，仍按次计量；实际GPU成本未知，不能称上游计算免费。MT Photos人脸与共享智能识别配置、本机doc-extract默认入口已切换；不代表其他外部消费者全部迁移。详情和证据边界见[开发文档](passthrough-development.md)。

## 在面板配置

进入「渠道」页底部的「透传路由」，点击「新建透传路由」。填写名称、路径前缀、完整上游地址及是否剥离前缀。例如前缀 `/face/`、上游 `http://inference.example:8066`，启用剥离前缀后，客户端 `/face/represent` 对应上游 `/represent`。

新建表单默认停用，且未授权任何 VKey、任何上游路径。配置完成并确认授权后再启用。

「允许调用的 VKey ID」填写密钥的数字 ID，以逗号或空格分隔；这不是密钥明文。白名单覆盖该Key本身及其有效后代，父Key撤销会阻止后代调用，且用户与全部父链的实时策略继续生效。留空表示全部拒绝。客户端使用获授权的 Tokengo VKey 鉴权（`Authorization: Bearer <VKey>`、`x-api-key`，或透传路由专用的 `api-key`；同一请求不能混用冲突凭据），不能用管理会话或运维令牌代替业务密钥。

已有 VKey 的渠道授权 `allowed_channels` 和付费开关 `allow_paid` 仍参与校验。给路由填写 VKey ID 不会自动扩大原权限：受限渠道的密钥还需允许路由对应的只读「计费接入 ID」，收费路由需要 `allow_paid=true`。建议分别为人脸、CLIP/OCR和文档解析创建独立VKey，设置 `allowed_models=[]` 阻止LLM调用，`allowed_channels`仅包含对应路由的计费接入ID，并用该路由的显式密钥白名单授予业务接口，并配置预算、RPM、并发限制，不设置 TPM。

「允许的上游路径」每行一个精确路径，填写剥离前缀后的最终上游路径，不含查询字符串。例如 `/represent`、`/clip/img`。留空表示全部拒绝；勾选「允许前缀下全部上游路径」则不限制子路径。建议明确列出业务接口，避免同时开放 `/restart` 等维护接口。路径授权不按 HTTP 方法区分。

默认不重试。实际转发的请求包含上游 4xx / 5xx，按次计量；已发出但中断且结果未知时，保守扣除预留金额，次数和 token 用量记为未知。

填写「每次价格（microUSD）」、超时、并发上限和请求体大小上限。表单默认价格为 0、超时 60 秒、并发上限 16、请求体上限 20 MiB。1 USD = 1,000,000 microUSD。用户预算、RPM 和并发限制继续生效；带 TPM 限制的密钥不能调用无法统计 token 的透传接口。

如果上游需要 `api-key` 等静态凭据，在「上游 Header」中添加。值输入框按密码处理。编辑时已有值留空会保留；填写新值会替换；移除整行会删除该 Header。已保存 Header 的名称不能直接修改，需要移除后重新添加。关闭编辑窗口后，尚未保存的输入值会从表单状态中清除。

列表的「检查」会打开手动诊断窗口，可指定已被路由白名单允许的上游路径、GET / POST / HEAD 方法与 200 至 599 的预期状态码。执行的是一次无正文请求，结果只显示状态和耗时，不回显响应正文，也不扣用户额度。默认 `/check` 仅是可编辑的起始值，应先确认上游确实提供安全的检查接口。通过检查不代表人脸检测或 CLIP 推理已经验收。

本版仅提供手动 HTTP 诊断，尚未接入定时探针调度。普通 HTTP 方法及正文可透传，不支持 CONNECT、WebSocket 或协议升级；不会保留 HTTP 线上的分块边界。对于 HTTP 客户端无法原样表示、会被自动规范化的路径或查询字符串，服务端返回 400，避免静默改变请求。

保存采用版本检查。发生配置冲突时，关闭编辑窗口并刷新列表，再重新修改，避免覆盖其他人的更新。删除路由需要再次确认，历史账目保留。

## 路径示例与迁移边界

前缀剥离只移除网关前缀，不会补出上游的业务路径：

| 网关前缀 | 上游原生路径 | 客户端完整路径（剥离前缀） |
| --- | --- | --- |
| `/face/` | `/represent` | `/face/represent` |
| `/clip/` | `/clip/img` | `/clip/clip/img` |
| `/clip/` | `/ocr` | `/clip/ocr` |

要避免第二个例子的重复路径，可以配置不同的网关前缀，或根据上游完整路径选择保留前缀；应先核对调用方实际发出的路径，不能只根据模型名称推测接口。

2026-10-21 首次现场检查中，CLIP 业务请求出现 CUDA 500，旧 LiteLLM 入口的两条常规路径返回 404。随后使用临时本地 Rust 网关、隔离身份和合成材料复测，Face 与 CLIP 的直连/透传响应均为 200，正文逐字节一致，两条手动诊断也通过。期间没有修复或重启生产服务，CLIP 先前的异常只能记录为间歇性现象，不能认定根因消除；旧 LiteLLM 路由仍不能作为已可用的迁移基线。

上述2026-10-21记录是历史阶段。2026-10-22已在正式配置上完成Face、CLIP文本/图像及OCR标记合成请求，直连与网关HTTP200且正文对等，新账完成；随后切换MT Photos相关配置。此前AI容器丢失GPU访问，本轮仅重启该AI容器一次恢复CUDA后重验OCR；这次恢复不能解释所有历史间歇性问题。尚无自然后台任务的验收证据，未触发图库重扫、清空向量或更改识别模型。其他消费者切换、长期观察、停用LiteLLM及GPU面板数据源联动仍须单独核对。

## MT Photos 调用方切换

分别为人脸服务、CLIP/智能识别服务和文档解析服务创建专用 VKey，每把仅授权自己的路由与计费接入 ID，设置 `allowed_models=[]`。上游静态凭据留在 Tokengo；调用方的地址和 `api_key` 要一起替换，不能继续拿后端凭据访问网关。

部分 MT Photos 版本使用 `FACE_REG_API_CONFIG` 保存人脸地址，使用 `OCR_API_CONFIG` 的同一组 `uri/api_key` 调用 CLIP 和 OCR。后者应保留 `/check`、`/clip/txt`、`/clip/img`、`/ocr` 的精确白名单；`b_uri/b_api_key` 是文本搜索备用接口，不能替代 CLIP 图片向量主入口。以实际版本的请求为准，不开放维护或重启路径。

例如网关基础地址是 `https://gateway.example` 时，人脸配置填写 `https://gateway.example/face`，共享智能识别配置填写 `https://gateway.example/clip`；客户端会继续拼接原生业务路径。保持原识别模型、向量维度、阈值、线程数及其他 LLM 来源配置。调用方已有多个工作线程时，用户/VKey 的并发上限须容纳它们，路由并发可用于排队保护上游。

先备份相关配置，逐条完成直连与网关合成业务验收，再使用应用原生保存接口切换；直接更新配置存储时，还须按该应用版本的机制重载运行配置。共享上游任一启用能力失败时，先处理该能力或暂停对应调用方切换，独立服务可继续。切换后核对应用实际请求及专用 Key 账本；无需重扫图库、清空向量或修改识别模型。

## 管理 API

本版仅允许 root 面板会话管理透传路由和执行诊断；现有运维令牌未获得新增透传管理能力。浏览器仍使用同源 HttpOnly 会话，写请求带现有的面板 CSRF 标记。

| 方法与路径 | 返回与约束 |
| --- | --- |
| `GET /api/passthrough-routes` | 路由数组；Header 仅返回 `{name, has_value}` |
| `POST /api/passthrough-routes` | 创建后返回路由对象 |
| `PUT /api/passthrough-routes/{id}` | 完整更新，必须包含读取所得 `revision`；版本冲突返回 409 |
| `DELETE /api/passthrough-routes/{id}?revision=N` | 按版本删除，历史账目保留 |
| `POST /api/passthrough-routes/{id}/probe` | 请求 `{path, method, expected_status}`；返回检查结果、HTTP 状态和耗时 |

路由写入字段为 `name`、`prefix`、`upstream`、`strip_prefix`、`enabled`、`price_micro_usd_per_request`、`timeout_secs`、`max_concurrent`、`max_body_bytes`、`allowed_key_ids`、`allowed_paths`、`headers`。写入 Header 使用 `{name, value}`，其中 `value: null` 仅表示保留同名已保存值，不能用来创建缺少值的新 Header；从列表移除名称表示删除。读取对象另含 `id`、`revision` 和只读的 `channel_id`，后者用于现有计费及渠道权限校验。

`allowed_key_ids: []` 表示拒绝全部 VKey。API 中 `allowed_paths: null` 表示全部子路径、`[]` 表示全部拒绝；面板新建表单额外选择安全的默认值 `[]`，需要主动填写路径或勾选全部路径。

## MinerU 同步接入

第一阶段仅代理兼容HTTP服务（默认端口5024）的 `/file_parse` 和 `/health`。上游示例为 `http://mineru-upstream.example:5024`，网关前缀可设 `/mineru/`，开启剥离前缀，使用专用VKey及精确路径白名单 `["/file_parse","/health"]`。上传字段为 `files`，结果为内联Markdown或原ZIP。

建议并发1–2，默认body上限20MiB（含multipart开销），可配置但不超过100MiB。覆盖兼容层900秒等待时，网关可设1000秒并同步调整调用方和反向代理，为上传、排队及返回留出余量。网关断开或超时不等于MinerU后台作业取消，不自动重试长任务。

健康GET和上传POST都必须携带网关VKey。数据面 `/mineru/health` 与解析在同一路由时按同一单价计次；root面板的手动probe记系统诊断、不扣用户预算，二者不能混为免费健康检查。同步接入不支持原生tasks/uploads/files/jobs异步链、动态ID路径ACL或返回URL改写；不要用允许全部路径替代专门设计。

可选标准库客户端为仓库 `scripts/mineru-gateway-client.py`，原生包位于 `tools/`；凭据仅通过 `TOKENGO_API_KEY` 或私有 `TOKENGO_API_KEY_FILE` 输入，健康和上传均鉴权、禁跳转，支持Markdown/ZIP输出。它不是Rust网关运行依赖。完整参数、接口边界和部署验收见[开发文档](passthrough-development.md)。

本机doc-extract已于2026-10-22改为专用网关默认入口、私有凭据文件及1050秒上传等待，健康GET和上传POST同时鉴权。10项客户端回归通过；Mac上真实合成PNG CLI调用返回 `mineru-gpu` 且包含预置标记，两笔网关请求均完成计量。此项是本机外部客户端的运维改动，不由本仓库分发，也未推广到其他机器或MCP入口；仓库提供的可选标准库客户端仍可独立使用。
