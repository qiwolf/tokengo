# 中性trace跨Messages兼容

2026-10-28。`request_id`只是有界追踪元数据，不能单凭其存在拒绝普通GLM→Messages路线，也不能用它替换response ID、tool call ID或会话身份。本次只扩展这一中性字段，不改变cache/usage语义或放开搜索、过滤、媒体等未知语义。

Chat/Responses保持既有顶层request_id。转换到Messages时使用明确TokenGo扩展 `tokengo_metadata: {request_id: string}`。JSON位于message对象顶层；SSE早到trace放在 `message_start.message.tokengo_metadata`，晚到trace放在原有 `message_delta.tokengo_metadata`，不创建额外event，不塞入content、stop_reason或tool input。1024字节/非空字符串限制保持；同值重复幂等，跨帧或legacy/namespace冲突拒绝。命名空间只接受request_id，不能夹带新语义字段。

Messages反向转换读取同一扩展并还原Chat/Responses顶层request_id；原消息和工具身份不由trace生成。原生Messages同协议JSON/SSE仍原样转发，包括未知native扩展；这不表示把这些扩展授予跨协议转换能力。复杂cache细分的具体不等价拒绝不在本次改动范围。

生产转换器回归入口 `provider_trace_messages`覆盖JSON双向、JSON/SSE工具身份、1/7/整块分帧、早/晚trace、冲突/非法类型/超长/语义扩张，以及原生Messages真实loopback HTTP的JSON/SSE原值保持。设置 `TOKENGO_PROVIDER_TRACE_EXPORT` 导出生产转换器实际生成的JSON、early SSE、late SSE，供SDK消费；不是手写输出冒称转换产物。

SDK锁定[anthropic-python 2.3.0官方PyPI元数据](https://pypi.org/pypi/anthropic/2.3.0/json)，wheel SHA256 `aa6a3984f9276f307933d42dc6f58b6efc821a5ecafbc8d1bb5993a066767622`，声明依赖及文件URL见 `crates/server/tests/fixtures/provider_compatibility/anthropic-sdk-lock.json`。执行点实际依赖版本由独立SDK结果回收，不能用依赖范围冒称安装锁。

`anthropic_trace_compare.py`只对loopback生产转换器导出结果做SDK容忍与raw HTTP对照：JSON extra可见；SSE message_delta event extra可见；SDK组装的text/tools/terminal/usage与raw事件一致。**不承诺**SDK `get_final_message()`将晚到未知扩展合并入最终Message快照；晚trace的读取位置明确是原始message_delta事件。

本材料不声称已持久化trace日志或已通过真实Anthropic/GLM账号验收。新的Rust和SDK结果由root在build-test执行并回收后单独记载；此处不预先填通过。
