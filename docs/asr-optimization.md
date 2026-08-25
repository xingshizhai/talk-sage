# TalkSage ASR 架构与优化

**最后更新：** 2026-08-25（架构迁移：VAD 切段 + GPU 加速 + 阿里云云端回退）

---

## 架构概述

TalkSage ASR 采用"VAD 切段 + 离线整段推理"架构（2026-08-25 迁移，替代原流式逐帧增量路径）：

```
麦克风 → Silero VAD 切段 → 段内累积音频
                              ↓ 段结束（静音超阈值）
                   [本地有 GPU] Qwen3-ASR / WhisperSmall (GPU) → 高精度文本
                   [无 GPU + 有 AccessKey] 阿里云 WebSocket → 高精度文本（200–500ms）
                   [无 GPU + 无 key]      WhisperSmall (CPU) → 本地兜底
```

**延迟取舍：** VAD 段结束后 1–3s（GPU 本地）或 200–500ms（阿里云云端）出文字，换取
识别精度从旧流式路径的 CER ~20% 提升到 <5%（与 noScribe/Meetily 同档）。

---

## 模型目录

| ID | 类型 | 路径 | 语言 | 推荐场景 |
|---|---|---|---|---|
| `qwen3-asr` | VAD 段级（默认） | `models/qwen3-asr/` | 中文/多语言 | 有 GPU 时首选 |
| `whisper-small` | VAD 段级 | `models/whisper-small/` | 多语言 | 无 GPU 本地兜底 |
| `whisper-base` | VAD 段级 | `models/whisper-base/` | 多语言 | 低配设备 |
| `paraformer-zh` | 流式（保留） | `models/paraformer-zh/` | 中文 | 不再作为默认路径 |
| `zipformer-en` | 流式（保留） | `models/zipformer-en/` | 英文 | 英文场景可选 |
| `aliyun-cloud` | 云端 WebSocket | 无本地模型 | 中文/英文 | 无 GPU + 有 AccessKey |

模型能力由 `talksage-asr::EngineKind::ALL` 和 `ModelProfile` 统一声明。`AliyunCloud` 不出现在用户可选模型列表（自动选择路径内部使用）。

---

## GPU 后端检测

`GpuBackend::detect()` 运行时自动检测：

| 后端 | 检测方式 | sherpa-onnx provider |
|---|---|---|
| `Cuda` | `dlopen nvcuda.dll`（Windows）/ `libcuda.so.1`（Linux） | `"cuda"` |
| `CoreMl` | `cfg(target_os = "macos")`（编译期） | `"coreml"` |
| `None` | 以上均不满足 | `"cpu"` |

`create_engine_auto(kind, model_dir, threads, gpu, options)` 自动将检测到的后端映射为 provider 并创建引擎。

`GET /api/asr/gpu_status`（headless）和 Tauri `get_gpu_status` 命令向前端暴露 `{backend, display_name, is_accelerated}`。

---

## 阿里云云端引擎

### 接入流程

1. 获取 Token：`http://nls-meta.cn-shanghai.aliyuncs.com/`（HMAC-SHA1 POP 签名）
2. 建立 WebSocket：`wss://nls-gateway-cn-shanghai.aliyuncs.com/ws/v1?token={token}`
3. 发送 `StartTranscription` JSON 消息
4. 循环推送 binary PCM 帧（16kHz mono int16 LE，建议 200ms 块）
5. 发送 `StopTranscription` → 等待 `SentenceEnd` 事件 → 关闭连接

### 配置

```toml
[asr]
asr_mode = "auto"                    # auto（推荐）| local | cloud
aliyun_access_key_id = "LTAI..."
aliyun_access_key_secret = "..."
aliyun_app_key = "..."               # NLS 项目 AppKey（实时语音识别）
```

所需 RAM 权限：`AliyunNLSFullAccess`

### 自动选择逻辑

```
cloud  →  AliyunEngine（强制云端）
local  →  create_engine_auto(gpu)（强制本地）
auto   →  无 GPU + 有 AccessKey + 有 AppKey  →  AliyunEngine
           其他                               →  create_engine_auto(gpu)
```

---

## 实时链路（双流）

```text
CPAL / WASAPI loopback
  → mono + 16 kHz
  → 500 ms pre-roll 缓冲
  → Silero VAD
  → 段内音频累积（accept()）
  → VAD 静音超阈值 → finish_speech()
  → engine.finish() → 整段文本
  → PunctuationRestorer → 标点恢复 + 语义分句
  → committed Segment → 插件 + SQLite
```

VAD 参数（`talksage.toml` 可调）：

| 参数 | 默认值 | 说明 |
|---|---|---|
| `redemption_ms` | 2000 | VAD 静音重判时间 |
| `pre_pad_ms` | 300 | 起音前填充（防首字截断） |
| `post_pad_ms` | 400 | 尾音后填充 |
| `min_speech_ms` | 250 | 最短有效语音段 |
| `min_commit_ms` | 400 | 最短提交时长（过滤噪音短段） |

---

## 标点恢复

`PunctuationRestorer` 在 `finish_speech()` 中调用（模型：`models/punct/`）：
1. 对整段文本做标点预测
2. 按语义句分拆为多个 `Segment`（避免超长段落）

标点恢复独立于 ASR 引擎，对本地 GPU 推理和阿里云云端结果均生效。

---

## 性能基准（CER / RTF）

使用 `talksage bench` 命令在 AISHELL-1 测试集评测：

```bash
cargo run -p talksage-asr --bin bench_cer --release -- \
  models/ path/to/aishell/wav/ path/to/aishell/transcript.txt \
  --engines qwen3-asr,whisper-small,whisper-base \
  --max 500
```

输出：各引擎 CER、RTF、最长推理延迟对比表。

---

## 引擎池（EnginePool）

`EnginePool` 按 `(kind, model_dir, options.signature())` 缓存引擎实例，跨监听会话复用（模型只加载一次）。

`options.signature()` 包含 `hotword_score|hotwords|provider`，确保 GPU 引擎与 CPU 引擎独立缓存，不互相污染。

---

## 延迟对比

| 路径 | 首字延迟 | 准确率（中文 CER） |
|---|---|---|
| 旧：Paraformer-zh 流式 | <500ms | ~15–25% |
| 新：Qwen3-ASR (GPU) | 1–3s（段结束后） | <5% |
| 新：WhisperSmall (CPU) | 2–5s（段结束后） | ~8–12% |
| 新：阿里云云端 | 200–500ms（段结束后） | <5% |

> CER 数字为估算，以实际 AISHELL-1 bench 结果为准。
