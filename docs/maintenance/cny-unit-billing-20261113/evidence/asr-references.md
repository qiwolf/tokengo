# ASR/TTS 官方接口与计量证据

读取日期：2026-11-13。以下为文档规则，不代表现有账号已经完成真实供应商验收；本次不调用生产 API。

- 硅基流动 ASR：<https://docs.siliconflow.cn/docs/api/audio-transcriptions-post>。原生端点 `/v1/audio/transcriptions`，multipart `file` + `model`；模型示例 SenseVoiceSmall/TeleSpeechASR。文档的成功响应只承诺 `text`，不承诺 `duration`。不能强行依赖响应包含时长，也不能将请求耗时当成音频时长。文档当前列明文件时长不超过一小时、大小不超过50MB；这些属于硅基流动当前规则，不硬编码为所有供应方规则。
- 硅基流动 TTS：<https://docs.siliconflow.cn/docs/userguide/capabilities/text-to-speech>。该指南“支持模型列表”处明确按输入文本长度对应 UTF-8 字节计费；不是 Unicode 字符数。因此通用计费单位需要独立的 `byte` 与 `character`，不可混同。
- 硅基流动 TTS 接口：<https://docs.siliconflow.cn/docs/api/audio-speech-post>，返回音频二进制。与 ASR 上传音频的按时长计量是不同的用量来源。

实现边界：ASR优先使用上游响应结构中的明确音频秒数，其次使用服务器检查的音频容器时长。纯Rust直接支持完整PCM/IEEE浮点WAV与完整MPEG Layer III帧的MP3（包含VBR）；其他格式可使用可选 `ffprobe`，通过stdin输入，不将音频落盘。缺少解码器或无法可靠读取时长时，保持用量未知，使用已配置数量上限预占，不伪造为零。上游未发出前失败释放，发出后不明状态保留未知结算。
