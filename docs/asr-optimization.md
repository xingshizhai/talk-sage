# TalkSage ASR 多模型与低延迟优化

## 目标

TalkSage 同时支持低延迟实时字幕和更高准确率的段级转写。模型选择必须来自统一目录，桌面端、Headless API 和 OpenAI 兼容 API 不再分别硬编码。

## 当前模型目录

| ID | 类型 | 速度等级 | 语言 | 使用建议 |
|---|---|---|---|---|
| `paraformer-zh` | 流式 | realtime | 中文 | 默认中文实时字幕 |
| `zipformer-en` | 流式 | realtime | 英文 | 默认英文实时字幕 |
| `whisper-base` | VAD 段级 | balanced | 多语言 | 准确率与速度平衡 |
| `whisper-small` | VAD 段级 | accurate | 多语言 | 准确率优先 |
| `qwen3-asr` | VAD 段级 | accurate | 多语言 | 模型完整安装后启用 |

模型能力由 `talksage-asr::EngineKind::ALL` 和 `ModelProfile` 统一声明。模型目录必须通过文件完整性检查才会在配置界面中启用。

## 实时路径

```text
CPAL / loopback
  -> mono + 16 kHz
  -> 500 ms pre-roll
  -> Silero VAD
  -> streaming ASR partial
  -> hybrid endpoint（VAD 或独立停顿）
  -> ASR final
```

VAD 需要累计 `min_speech` 才确认起音。确认之前的最近 500 ms 音频会回放给 ASR，防止普通话首字被截断。pre-roll 不增加模型推理等待时间，只增加约 32 KB/流的 f32 缓冲。

实时流式模型使用可独立于 VAD 的混合端点：文本稳定且连续安静约 450ms 时提交；即使 hypothesis 仍变化，连续安静约 850ms 也强制提交；不足 1 秒的短段受最短时长保护。主动提交后重置 Silero 状态，避免旧段尾污染下一句。文件导入和固定语料评估仍只使用确定性 VAD 边界。该机制不反复重跑音频窗口，因此不会显著增加推理量。

端点参数位于 `audio.endpoint`：`stable_ms`、`quiet_ms`、`force_quiet_ms`、`quiet_rms`、`min_segment_ms`，也可在设置页调整。

## 配置语义

- `asr.user_engine` / `asr.client_engine`：生活、会议、会谈等内置场景使用的模型。
- `scene.custom.user_engine` / `client_engine`：仅自定义场景使用。
- 实时模型持续产生 partial；段级模型只在 VAD 结束后产生 final。
- 未安装或文件不完整的模型不能开始识别，客户流可安全降级关闭。

## 延迟策略

1. 默认选择 `realtime` 模型，保证交互字幕速度。
2. `balanced` 和 `accurate` 模型明确展示为段级，避免用户误以为支持逐字输出。
3. 所有引擎通过 `EnginePool` 跨会话复用，避免重复加载；离线大模型每种最多缓存一个，流式模型最多缓存四个。
4. 后续可增加“双通道”模式：Paraformer 输出 partial，SenseVoice/Whisper 在段结束后输出 revision final；这需要先完善 revision 替换语义和 CER 基准。

## 评测要求

模型升级以真实录音集为准，至少记录：中文 CER、首个 partial 延迟、final 延迟、RTF、内存峰值。现有“输出非空”集成测试只能证明模型可运行，不能作为准确率结论。
