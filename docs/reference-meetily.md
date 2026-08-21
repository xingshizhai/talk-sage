# 参考项目研究：Meetily（Zackriya-Solutions/meetily）

> 调研日期：2026-08-20。来源：`D:\Work\github\meetily`（MIT，v0.4.0，~524 文件）。与 talk-sage 定位几乎相同（本地优先会议助理），借鉴价值最高。

## 一、项目定位

**隐私优先（Privacy-First）的本地会议 AI 助手**：在用户自己的设备上实时捕获会议音频、实时转写、调用 LLM 生成纪要，**全程数据不出本机**。

- 解决问题：云端会议转写工具的隐私与合规风险（面向高管、法律/医疗/国防等敏感行业）
- 核心卖点：① 本地转写（whisper.cpp / NVIDIA Parakeet ONNX，GPU 加速 Metal/CUDA/Vulkan）② 麦克风+系统音频**双流采集**与智能混音 ③ AI 纪要（Ollama 本地推荐 + 各家云端 + 内置 llama-helper sidecar 离线）④ **崩溃恢复**（增量 checkpoint + IndexedDB）⑤ 跨平台
- 注意：Community 免费开源，Enterprise 收费（含 diarization）；借鉴了 whisper.cpp / screenpipe / transcribe-rs

## 二、技术栈

| 层 | 选型 |
|---|---|
| 桌面壳 | Tauri 2.6.2（单实例/托盘/通知/updater/store 插件）|
| 前端 | Next.js (App Router) + React + Tailwind + shadcn/ui |
| ASR | whisper.cpp（whisper-rs 0.13.2 raw-api）+ NVIDIA Parakeet（ort ONNX Runtime）|
| VAD | Silero VAD（silero_rs）|
| 音频 | cpal（WASAPI 等）、rubato 重采样、nnnoiseless（RNNoise）、ebur128、symphonia、ffmpeg-sidecar |
| 存储 | SQLite（sqlx 0.8）+ 每会议文件夹 + 前端 IndexedDB 缓冲 |
| 纪要 LLM | Ollama/OpenAI/Anthropic/Groq/OpenRouter/自定义端点/内置 llama-helper sidecar |
| 并发 | tokio + rayon + crossbeam + dashmap |

## 三、架构设计

```
麦克风流 ─┐
          ├─ cpal 回调 → 声道→mono、持久化 rubato 重采样→48kHz（处理蓝牙 8/16/44.1kHz 变采样率）
系统回流 ─┘        ├─ 仅麦克风：80Hz 高通 → RNNoise 降噪 → EBU R128(-23 LUFS) 归一
                  └─ AudioChunk → mpsc channel
                                      ▼
              AudioPipeline::run（常驻 task）
                  ├─ 环形缓冲 add_samples → extract_window（600ms 窗口，不足补零）
                  ├─ ProfessionalAudioMixer::mix_window（比例软缩放防削顶）
                  ├─ ContinuousVadProcessor（Silero，16kHz，redemption 400ms/2000ms）
                  │      └─ 段长 ≥50ms 才打包 → transcription_sender
                  └─ 混合后音频 → recording_sender（录音文件）
                                      ▼
              start_transcription_task（worker.rs，单 worker 串行保证时序）
                  ├─ Whisper | Parakeet | Provider(trait)
                  ├─ 置信度门槛：Whisper≥0.3 才采纳
                  └─ emit "transcript-update" {sequence_id, audio_start/end_time,
                       duration, confidence, is_partial}
                                      ▼
              后端同事件落库（upsert 覆盖同 sequence_id）+ 前端实时渲染/打字机
```

关键组件：
- **RecordingState**：Arc 共享录制状态（时长/设备/错误上报 `user_message()`+`is_recoverable()`），流/管线/转录 task 的同步中枢
- **ContinuousVadProcessor**：30ms 分块喂 Silero；redemption 400ms（实时）/2000ms（批处理）桥接停顿；min_speech 250ms；前后 padding；flush 强制收尾
- **TranscriptionProvider trait**：`transcribe(audio, language) -> {text, confidence, is_partial}`，Whisper/Parakeet 统一接口
- **WhisperEngine**：模型 catalog 12 个、大小+GGML magic 损坏校验、HF 下载带进度/取消、GPU 后端探测
- **SummaryEngine**：长转写分块各自总结→合并→最终报告 + 语言归一/翻译二次 pass + 取消令牌
- **前端**：RecordingStateContext（状态机+500ms 轮询双通道）、TranscriptContext（IndexedDB 缓冲）、useTranscriptStreaming（打字机）

## 四、优点

1. **VAD 参数工程化 + 行为测试**（vad.rs）：redemption 实时 400ms/批处理 2000ms、min_speech 250ms、前后 padding、短段 RMS/Peak 能量过滤防幻觉；**5 个单元测试把分段质量变成可回归契约**（talk-sage 最缺）
2. **专业音频链路**：600ms 环形缓冲窗口对齐混合 + 软缩放防削顶；持久化重采样器解决蓝牙变采样率 bug（快进 3 倍、能量放大 173%）；设备解析阶梯 + macOS 蓝牙 override
3. **事件驱动负载设计**（TranscriptUpdate）：sequence_id + audio_start/end_time + duration + confidence + is_partial，前端时间轴/播放器跳转/打字机，后端同事件落库，生产消费解耦、刷新可重放
4. **"零 chunk 丢失"收尾**：input_finished 标志 + 关闭通道 + 结束前比较 queued/completed（重试 10 次）+ 丢失事件；force_flush 连发特殊 chunk_id 冲刷 VAD 残余防"停止丢尾"
5. **多级崩溃恢复**：音频 30s checkpoint → ffmpeg concat `-c copy` 免重编码合并；每条 segment 增量写 transcript.json + 前端 IndexedDB；启动检测可恢复会议弹窗
6. **模型生命周期管理**：catalog + 大小/magic 校验 + 下载进度/取消 + 录制前 `validate_transcription_model_ready` 强制校验

## 五、不足

1. **历史包袱严重**：audio/core-old.rs、*.backup、新老两套音频系统（audio_v2 未接线）、三套转写路径并存、遗留 Python FastAPI 后端
2. **全局 static 可变状态 + 原子标志泛滥**：static RECORDING_MANAGER/TRANSCRIPTION_TASK + 多个 AtomicBool + `unsafe impl Send` + `static mut SAMPLE_COUNTER`，单元测试几乎无法覆盖
3. **"表演性并行"**：worker 实为 NUM_WORKERS=1 串行，却 emit "workers: 3"；parallel_processor 未整合；逐 chunk info 日志与"只记每 10 chunk"注释矛盾
4. **双存储一致性复杂**：IndexedDB + SQLite + sessionStorage 三处，停止失败可能出现分裂状态，靠 useTranscriptRecovery 打补丁
5. **死代码/占位**：get_transcription_status 返回写死值、日志目录提交进 git、遗留 backend/ 与 README 主链路不符

## 六、对 talk-sage 的可借鉴点（按优先级）

1. **VAD 调参与分段质量测试（最高优先）**：sherpa-onnx 的 VAD 同为 Silero 系，redemption 400ms/2000ms、min_speech 250ms、前后 padding、短段能量过滤的思路与 5 个测试原样迁移——"用测试锁定分段行为"
2. **转录结果负载结构（直接采用）**：`{sequence_id, audio_start/end_time, duration, confidence, is_partial}` + 事件广播 + 后端同事件落库（upsert 同 sequence_id）——talk-sage 流式 sherpa-onnx 结果天然带时间戳，封装后同时满足实时 UI/时间轴/SQLite/刷新恢复
3. **录音双流混合 + 增量 checkpoint 崩溃恢复**：600ms 环形缓冲窗口混合；"30s checkpoint → ffmpeg concat 免重编码合并"；force_flush 冲刷 VAD 残余的收尾协议（talk-sage 已实现跨流去重，可加 flush 收尾）
4. **引擎抽象 + 配置驱动选择**：`StreamingEngine`/`OfflineEngine` 两个 trait（同构 transcribe/is_model_loaded/get_current_model）+ 段级结果统一负载；录制前 validate_model_ready 避免"录完才发现模型没下好"
5. **设备解析阶梯 + 采样率自适应**：偏好→默认→报错/继续阶梯；持久化重采样器解决回环变采样率（Windows 回环同样可能 44.1k/48k 不定）；蓝牙 vs 有线影响缓冲时长
6. **前端状态机与双通道同步**：RecordingStatus 生命周期（idle→starting→recording→stopping→processing→saving→completed/error）+ 事件+500ms 轮询双通道，解决刷新失步
7. **纪要分块→合并管线**：分块各自总结→合并→最终报告 + 语言归一/二次 pass + `<think>` 清洗 + 取消令牌 + JSON 模板——talk-sage 接任意 LLM 端点可复用
8. **小技巧**：模型 catalog + 大小/magic 校验；错误类型带 user_message/is_recoverable（前端决定弹窗/toast）；"会议文件夹 + metadata.json + transcript.json + audio.mp4" 磁盘结构；录制开始前 engine lifecycle lock 防并发竞争

**不建议借鉴**：全局 static 状态（改用注入 AppState）、双存储一致性（talk-sage 直接 SQLite 单写）、多套体系并存、逐 chunk 高频 info 日志。

## 七、总结

Meetily 的架构价值不在"新颖"，而在**把本地实时转写的工程坑（VAD 碎片化、蓝牙采样率、停止丢尾、崩溃丢数据、模型未就绪）逐个用可测试的机制填平**——这些坑正是 talk-sage 同赛道必然要踩的。其"VAD 调参 + 事件负载 + 增量恢复 + 设备阶梯 + 前端状态机"五件套值得整体移植，与 talk-sage 现有架构（双流/VAD/sherpa-onnx/SQLite）高度互补。
