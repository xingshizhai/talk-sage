# TalkSage ASR 架构与优化

**最后更新：** 2026-08-27（Apple Metal whisper.cpp 在 M4 上实测通过）

---

## 架构概述

TalkSage ASR 采用"VAD 切段 + 离线整段推理"架构（2026-08-25 迁移，替代原流式逐帧增量路径）：

```
麦克风 → Silero VAD 切段 → 段内累积音频
                              ↓ 段结束（静音超阈值）
                   [NVIDIA CUDA] Qwen3-ASR 0.6B → 高精度文本
                   [Apple Metal] Whisper large-v3-turbo Q5_0（whisper.cpp）
                   [无 GPU + 有 AccessKey] 阿里云 WebSocket → 高精度文本（200–500ms）
                   [无受支持 GPU]         阿里云 WebSocket（必须配置完整凭证）
```

**延迟取舍：** VAD 段结束后 1–3s（GPU 本地）或 200–500ms（阿里云云端）出文字，换取
识别精度从旧流式路径的 CER ~20% 提升到 <5%（与 noScribe/Meetily 同档）。

---

## 模型目录

| ID | 类型 | 路径 | 语言 | 推荐场景 |
|---|---|---|---|---|
| `qwen3-asr` | VAD 段级 | `models/sherpa-onnx-qwen3-asr-0.6b/` | 中文/多语言 | CUDA；显式 CPU 诊断 |
| `whisper-large-v3-turbo-metal` | VAD 段级 | `models/whisper.cpp-large-v3-turbo-q5_0/` | 多语言 | Apple Silicon Metal，可运行 |
| `punct` | 辅助模型 | `models/punct-ct-transformer/model.onnx` | 中文/英文 | 标点恢复与语义分句 |
| `aliyun-cloud` | 云端 WebSocket | 无本地模型 | 中文/英文 | 无 GPU + 有 AccessKey |

模型能力由 `talksage-asr::EngineKind::ALL` 和 `ModelProfile` 统一声明。产品目录只包含 Qwen3-ASR 0.6B 与 Whisper large-v3-turbo Q5_0 Metal；Paraformer、Zipformer、旧 sherpa ONNX Whisper 不再出现在模型管理或设置中，仅保留内部解析用于自动化测试。`ModelProfile::selectable` 按编译平台暴露能力：Apple Silicon 构建启用 Metal 引擎，其他平台不把该模型放进引擎选择框。

模型管理细节、状态机、下载源与完整性规则见 [模型管理架构](model-management.md)。

模型下载全生命周期写入应用日志：任务提交、模型目录、空间预算、下载源、续传 offset、每 10% 进度（服务端不提供总大小时每 64 MiB）、解压、SHA-1 校验、完成、取消与错误。进度日志在底层下载器统一节流，桌面端和 headless 入口只补充任务边界，因此不会因 256 KiB 下载块产生大量重复日志。

---

## GPU 后端检测

`GpuBackend::detect()` 运行时自动检测：

| 后端 | 检测方式 | 实际推理 |
|---|---|---|
| `Metal` | macOS **aarch64**（Apple Silicon） | whisper.cpp `use_gpu=true`；native 日志须出现 `using Metal backend` |
| `Vulkan` | Windows x64 且以 `vulkan-gpu` feature 编译，并能加载 `vulkan-1.dll` | 同一套 whisper.cpp adapter |
| `Cuda` | `dlopen nvcuda.dll`（Windows）/ `libcuda.so.1`（Linux） | sherpa-onnx Qwen3-ASR |
| `None` | 以上均不满足 | CPU（仅 `asr_mode=local`）或阿里云 |

`resolve_asr_route()` 是路由的单一事实来源：`auto` 模式在检测到 CUDA / Metal / Vulkan 时走本地 GPU；否则要求完整阿里云凭证。`local` 可显式 `cpu` / `cuda` / `metal` / `vulkan`。Apple GPU **不**走 sherpa `provider=coreml`（该路径会 `Fallback to cpu!`）。Intel GPU 尚未实现。

路由确定后 provider 写入 `EngineOptions` 再进 `EnginePool`。whisper.cpp Metal/Vulkan 与 sherpa CPU/CUDA 实例不会混用。

`GET /api/asr/gpu_status`（headless）和 Tauri `get_gpu_status` 暴露 `{backend, display_name, hardware_candidate, is_accelerated, effective_route}`。

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
local  →  按 backend 偏好走本地（auto 时用 detect() 结果）
auto   →  NVIDIA CUDA                         → 本地 Qwen3-ASR
          Apple Silicon Metal                 → 本地 Whisper large-v3-turbo Q5_0
          Windows Vulkan                      → 同上 whisper.cpp GPU
          无受支持 GPU + 完整三项凭证        → AliyunEngine
          无受支持 GPU + 凭证不完整           → 启动前返回配置错误
```

### Apple Silicon 实测与模型决策（M4 / 16GB）

sherpa-onnx 1.13.5 macOS arm64 静态库**不能**当 Apple GPU 用：同一 Paraformer、同一段 10.05s 音频，CPU RTF 0.064，`provider=coreml` RTF 0.065，native 打印 `Fallback to cpu!`。因此禁止把 Qwen3-ASR 配成 Apple GPU 默认模型。

产品路径是独立的 **whisper.cpp + Metal** adapter（`crates/talksage-asr/src/metal.rs`，`whisper-rs` feature `metal`）。`asr_mode=auto` 在 Apple Silicon 上路由到 `whisper-large-v3-turbo-metal`。

**2026-08-27 本机复测（Apple M4 16GB，macOS 26.5.1，whisper.cpp 1.8.3，debug CLI）：**

```bash
python3 scripts/evaluate.py asr --engines whisper-large-v3-turbo-metal
```

| 项 | 结果 |
|---|---|
| native GPU | `ggml_metal_device_init: GPU name: Apple M4`；`whisper_backend_init_gpu: using Metal backend` |
| 权重量 GPU | `whisper_model_load: Metal total size = 573.40 MB` |
| 模型加载 | 首次 390–675 ms；引擎池复用后约 8–10 ms |
| 段级推理 RTF | 英文 6.4s 段 **0.32**；中英混合三段 **0.56–0.81**（均 < 1） |
| 管道实时系数 | **1.29**（含按墙钟喂 wav，不是纯 GPU RTF） |
| 启动语料 CER | 英文 **2.9%**，中英混合 **11.8%**，两条平均 **7.3%**（门禁通过，推荐该引擎） |
| 稳定性 | `GGML_METAL_NO_RESIDENCY=1` 关闭 residency set，不关闭 Metal |

设置页同时展示「物理硬件」和「当前推理后端」，避免把 M4 GPU 与 sherpa CPU 混为一谈。Qwen3-ASR 0.6B 仍留给 CUDA 与显式 CPU 对比。
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

`PunctuationRestorer` 在 `finish_speech()` 中调用（模型：`models/punct-ct-transformer/model.onnx`）：
1. 对整段文本做标点预测
2. 按语义句分拆为多个 `Segment`（避免超长段落）

标点恢复独立于 ASR 引擎，对本地流式/离线推理和阿里云云端结果均生效。
当前公开模型是 sherpa-onnx `vocab272727-2024-04-12`，约 281 MiB；GitHub Release 为主源，Hugging Face 为备用源。旧 `vocab500k` 地址无效。

---

## 性能基准（CER / RTF）

使用 `talksage bench` 命令在 AISHELL-1 测试集评测：

```bash
cargo run -p talksage-asr --bin bench_cer --release -- \
  models/ path/to/aishell/wav/ path/to/aishell/transcript.txt \
  --engines qwen3-asr \
  --max 500
```

输出：各引擎 CER、RTF、最长推理延迟对比表。

---

## 引擎池（EnginePool）

`EnginePool` 按 `(kind, model_dir, options.signature())` 缓存引擎实例，跨监听会话复用（模型只加载一次）。

`options.signature()` 包含 `hotword_score|hotwords|provider`，确保 GPU 引擎与 CPU 引擎独立缓存，不互相污染。云端连接型引擎不进入本地模型池。

设置页和 `/api/asr/gpu_status` 同时显示“硬件候选”和“当前生效路线”。硬件候选表示平台/驱动探测结果；最终是否能创建该 provider 仍以监听启动时的模型与 ONNX Runtime 校验为准。

---

## 延迟对比

| 路径 | 首字延迟 | 准确率（中文 CER） |
|---|---|---|
| 旧：Paraformer-zh 流式 | <500ms | ~15–25% |
| 新：Qwen3-ASR (GPU) | 1–3s（段结束后） | <5% |
| large-v3-turbo Q5_0（M4 Metal，2026-08-27） | 段级推理 RTF 0.32–0.81；管道（含喂文件）约 1.29 | 启动语料英文 CER 2.9%、中英混合 CER 11.8%、平均 7.3% |
| 新：阿里云云端 | 200–500ms（段结束后） | <5% |

> 启动语料 CER 来自 2026-08-27 M4 实测（见上节）；AISHELL-1 全量仍以 `talksage bench` / `bench_cer` 为准。

## 待定：出字延迟的取舍（2026-08-28）

管道重构后卡顿已消除（ASR / 标点 / 声纹都不在主线程上跑了），剩下的是**固有延迟**：
whisper 是段级引擎、没有 partial，一句话要等整段说完（`force_segment_ms` 默认 8s）
再推理（8s 音频约 2.6s）才出字，最坏 10 秒才看到这句。

两条可选路径，尚未决定：

| 方案 | 出字延迟 | 代价 |
|---|---|---|
| `force_segment_ms` 8s → 3~4s | 砍半 | 上下文变短、准确率略降；硬切句子更频繁 |
| 改用流式引擎（paraformer-zh） | 几乎无延迟（边说边出 partial） | 中文准确率明显不如 whisper large |

补充事实：
- `force_segment_ms` 目前在 `service.rs` 里硬编码（流式 0 / 段级 8000），未暴露到设置页。
  真要调，顺手把它做成配置项更合适。
- 强制切分是按时长硬切，不是句子边界，切出来的文本会把两句话接在一起，
  下游的要点/术语提取拿到的也是半截句子。缩短阈值会放大这个问题。
