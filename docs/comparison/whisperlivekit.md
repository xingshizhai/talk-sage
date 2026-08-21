# 参考项目研究：WhisperLiveKit（WLK）

> 调研日期：2026-08-19。来源：`D:\Work\github\WhisperLiveKit`（Apache-2.0，Python）。

## 一、项目定位

**自托管、超低延迟的语音转写管道（服务端）+ 多客户端**：

- 后端：Python 服务（多用户并发 WebSocket），任意设备/浏览器访问
- 客户端：Web UI、**macOS 原生 SwiftUI 客户端**（AudioCapture + WebSocketTransport）、Chrome 扩展
- 主打"同时转写"研究策略：Simul-Whisper（AlignAtt policy）、WhisperStreaming（LocalAgreement policy）、Qwen3-ASR-causal（流式编码）
- 附加能力：NLLB 200 语言同传翻译、**NVIDIA NeMo Sortformer 流式说话人分离**（4 说话人）

## 二、架构

```
客户端音频（mic / 文件 / Chrome 扩展 tab audio）
  → WebSocket（/asr，per-session：language / target_language / mode / token）
  → audio_processor（silero VAD → ASR 合并(coalescing) → 增量提交）
  → 引擎（转写 / 同传翻译 / 流式 diarization，多后端：PyTorch / MLX / CTranslate2 / vLLM）
  → 增量结果（full / diff 协议）→ WS 推送 → 客户端渲染
```

- OpenAI 兼容 REST `/v1/audio/transcriptions`、Deepgram 兼容 WS、原生 WS 三套 API
- 工程完备：Docker（CPU/CUDA）、模型管理 CLI（pull/rm）、benchmark（WER/RTF/延迟散点）、指标收集、fixtures 测试

## 二·补 架构图（architecture.png）详解

架构图由 `scripts/generate_architecture.py` 绘制（20×12 三列布局），内容如下：

### 左列：FastAPI Server（绿）
- 入口：**Web UI**（HTML+JS）、**Frontend（可选）**
- API 面：`WS /asr` • `/v1/listen`、`REST /v1/audio/transcriptions`、`Health` • `/v1/models`
- 客户端：Browser / OpenAI SDK / Deepgram SDK / TestHarness

### 中列：Audio Processor —— **每会话**（蓝，每用户独立）
```
FFmpeg Decoding → Silero VAD（speech/silence）
  → SessionASRProxy（线程安全，每会话语言覆盖）
  → DiffTracker（可选 ?mode=diff，只推增量） + Result Formatter（FrontData.to_dict()）
  → 流式策略层：LocalAgreement (HypothesisBuffer)  |  SimulStreaming (AlignAtt for Whisper)
```
- 数据流：`PCM audio →` 引擎；`ASRTokens ←` 引擎（双向箭头）

### 右列：TranscriptionEngine —— **单例**（红，跨会话共享，模型只加载一次）
**6 类 ASR 后端**：
| 后端 | 流式模式 | 说明 |
|---|---|---|
| Faster-Whisper / MLX Whisper / OpenAI Whisper(云) | 分块（chunk-based） | PCM→Encoder→Decoder→Tokens；配 LocalAgreement 或 AlignAtt；语言检测+缓冲裁剪；CPU/CUDA/MLX |
| Voxtral MLX（Apple Silicon）/ Voxtral HF | **原生流式** | 增量编码器→自回归解码器、滑动 KV cache、逐 token 输出、**无需分块**；4B 参数、15 语言、6-bit 量化 |
| Qwen3 ASR 1.7B/0.6B + Qwen3 Simul + Forced Aligner | 批 + 对齐器 | ForcedAligner 提供**词级时间戳**；LocalAgreement 或 border-distance 策略；29 语言 |
| OpenAI API（云） | 远程 | 需 API key |

**Shared Components（共享服务）**：Mel Spectrogram（缓存 DFT+滤波器组）、**Diarization（Sortformer / pyannote）**、**Translation（NLLB • CTranslate2）**、`WhisperLiveKitConfig`（单一配置源）、TestHarness、Benchmark（8 语言 × 13 样本）

底部图例：三种流式模式 —— 原生流式（Voxtral，粉）/ 分块（Whisper，紫）/ 批+对齐器（Qwen3，绿）

### 架构设计要点（对 talk-sage 的启示）
1. **引擎单例、模型只加载一次**：跨会话复用 ASR 引擎；我们每次"开始监听"都重建 pipeline（模型加载 ~1.6s）——可借鉴"引擎常驻 + warmup"
2. **会话状态与计算解耦**：VAD/缓冲/语言覆盖在每会话处理器，ASR 计算在共享引擎 → 天然支持多用户
3. **流式策略独立成层**：LocalAgreement / AlignAtt 是"何时提交"的策略，与具体模型解耦（Voxtral 用自带策略）——我们的 VAD 分段+partial/final 是固定策略，可抽象
4. **共享组件独立化**：Diarization / Translation / Mel 作为共享服务——我们已有（wespeaker 声纹、LLM 翻译），但耦合在插件层
5. **DiffTracker**：增量传输协议——我们事件流（partial/final）已是增量思想

## 三、关键技术（与 talk-sage 对照）

### 1. 说话人分离 —— 最大借鉴点
- WLK：**Sortformer 流式 diarization**（`nvidia/diar_streaming_sortformer_4spk-v2`），无监督、无需注册：
  - 流式状态：speaker embedding FIFO + 全程 speaker cache + 说话人置换对齐
  - `DEV_NOTES.md` 记录 **4→2 说话人约束算法**：取 top-2 预测、动态映射，把 4spk 模型约束为 2 人场景
- 我们（talk-sage）：wespeaker 声纹 + **主人注册** + 在线聚类（客户1/2…），需要先注册主人
- 差异：WLK 免注册自动分人（靠模型），我们靠注册+聚类（轻量、可控）
- **借鉴方向**：① sherpa-onnx 已有 `offline_speaker_diarization` 支持（segmentation+clustering），可作为"免注册分人"的离线补充；② 借鉴 4→2 约束思路，把我们的"主人+其他"二元判定做得更稳

### 2. 增量提交策略（AlignAtt / LocalAgreement）
- WLK 用研究级策略解决"Whisper 切块丢词"；我们用 sherpa **streaming** 模型天然增量（partial→final），无此问题
- 可借鉴：`should_defer_inference`（ASR 合并/延迟阈值，`asr_coalesce_min_s`）——把过短音频攒一攒再识别，减少无效段；我们已有 VAD 分段，可加"最小提交时长"参数

### 3. 服务端协议与多会话
- WLK：多用户并发会话 + **diff 协议**（只推增量而非全量快照）
- 我们：单管道 + 事件流（partial/final 本就是增量，思想一致）；未来 headless 多用户时可直接参考其会话代理（`session_asr_proxy`）与 diff 设计
- 可借鉴：**OpenAI 兼容转写接口**（对外提供 `/v1/audio/transcriptions`），让 talk-sage 服务可被任意 OpenAI SDK 调用

### 4. 评测体系（benchmark）—— 实用借鉴
- WLK：benchmark runner + 数据集（acl6060 等）+ **WER / RTF（实时率）/ 延迟** 指标 + 散点图 + robustness（clean vs other 噪音对比）
- 我们：已有"录音→裁剪→回放验证"闭环与真实模型集成测试
- **借鉴方向**：在 `scripts/recording_loop.ps1` 基础上加**固定语料评测**：对标准 wav 集（含噪音/多人）跑转写，输出 WER + 实时率 + 首词延迟统计，沉淀为 talk-sage 的转写质量基准（与质量评估 meta 结合）
- ✅ **已落地**：`talksage bench --dir <语料> --engine paraformer-zh|zipformer-en [--limit N]`——`*.wav` + 同名 `.txt` 参考文本，引擎池热启动逐文件转写，输出 **CER/WER%（中文字符级/英文词级）+ RTF + 首词延迟** 与均值（core::cer/wer + edit_distance 有单测）。后续可扩展散点图与 clean/other 对比。

### 5. 指标收集
- WLK：metrics_collector 每会话指标 → 我们已有 SessionStats/质量评估，可补充 **RTF（转写耗时/音频时长）** 与端到端延迟两个指标到 meta

### 6. macOS 原生客户端
- WLK 用 SwiftUI 原生客户端；我们 Tauri 跨平台已覆盖，无需重复

## 四、结论与建议优先级

| 建议 | 优先级 | 说明 |
|---|---|---|
| 固定语料转写评测（WER/RTF/延迟基准） | 高 | ✅ 已实现 `talksage bench`（见 §四 4） |
| OpenAI 兼容转写 API（headless） | 中 | ✅ 已实现 `POST /v1/audio/transcriptions` + `GET /v1/models`（见 architecture-v2 §18.6） |
| 最小提交时长 / ASR 合并参数 | 中 | ✅ 已实现 `audio.min_segment_ms`（最短提交时长，短段丢弃；见 architecture-v2 §18.7） |
| 免注册说话人分离（sherpa diarization） | 低（大项） | 与现声纹方案互补，评估后决定 |
| RTF/延迟指标入 meta | 低 | 小改动，丰富历史回溯 |

WLK 与 talk-sage 定位不同（WLK=通用服务端转写管道，talk-sage=个人会议助理），但"说话人分离、增量提交、评测体系"三方面经验可直接借鉴。
