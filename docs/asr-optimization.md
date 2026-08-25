# TalkSage ASR 架构与优化

**最后更新：** 2026-08-25（ASR 路由、产品模型目录与模型管理下载架构）

---

## 架构概述

TalkSage ASR 采用"VAD 切段 + 离线整段推理"架构（2026-08-25 迁移，替代原流式逐帧增量路径）：

```
麦克风 → Silero VAD 切段 → 段内累积音频
                              ↓ 段结束（静音超阈值）
                   [NVIDIA CUDA] Qwen3-ASR 0.6B → 高精度文本
                   [Apple Metal] Whisper large-v3-turbo Q5_0（adapter 待接入）
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
| `whisper-large-v3-turbo-metal` | VAD 段级 | `models/whisper.cpp-large-v3-turbo-q5_0/` | 多语言 | Apple Metal，当前仅预下载 |
| `punct` | 辅助模型 | `models/punct-ct-transformer/model.onnx` | 中文/英文 | 标点恢复与语义分句 |
| `aliyun-cloud` | 云端 WebSocket | 无本地模型 | 中文/英文 | 无 GPU + 有 AccessKey |

模型能力由 `talksage-asr::EngineKind::ALL` 和 `ModelProfile` 统一声明。产品目录只包含 Qwen3-ASR 0.6B 与 Whisper large-v3-turbo Q5_0 Metal；Paraformer、Zipformer、旧 sherpa ONNX Whisper 不再出现在模型管理或设置中，仅保留内部解析用于自动化测试。`ModelProfile::selectable` 把“允许预下载”和“已经可以运行”分开，Metal adapter 完成前不会把新模型放进引擎选择框。

模型管理细节、状态机、下载源与完整性规则见 [模型管理架构](model-management.md)。

模型下载全生命周期写入应用日志：任务提交、模型目录、空间预算、下载源、续传 offset、每 10% 进度（服务端不提供总大小时每 64 MiB）、解压、SHA-1 校验、完成、取消与错误。进度日志在底层下载器统一节流，桌面端和 headless 入口只补充任务边界，因此不会因 256 KiB 下载块产生大量重复日志。

---

## GPU 后端检测

`GpuBackend::detect()` 运行时自动检测：

| 后端 | 检测方式 | sherpa-onnx provider |
|---|---|---|
| `Cuda` | `dlopen nvcuda.dll`（Windows）/ `libcuda.so.1`（Linux） | `"cuda"` |
| `CoreMl` | 预留枚举；当前产品构建不返回该值 | `"coreml"` |
| `None` | 以上均不满足 | `"cpu"` |

`resolve_asr_route()` 是路由的单一事实来源：当前 `auto` 模式只有经过运行时确认的 CUDA 才走 sherpa 本地，其他机器要求阿里云；`local` 模式保留显式 CPU 作为离线、隐私和诊断选项。Apple GPU 不通过 sherpa provider 路由，而由后续独立 Metal adapter 承载。Intel GPU 尚未实现。

路由确定后 provider 会先写入 `EngineOptions`，再进入 `EnginePool`。因此缓存键和实际 ONNX Runtime provider 一致，CPU/CUDA/CoreML 模型实例不会混用。

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
auto   →  已验证可用的 NVIDIA CUDA          → 本地 Qwen3-ASR
          无受支持 GPU + 完整三项凭证        → AliyunEngine
          无受支持 GPU + 凭证不完整           → 启动前返回配置错误
```

### Apple Silicon 实测与模型决策（M4 / 16GB）

本机使用 sherpa-onnx 1.13.5 macOS arm64 静态库，以同一 Paraformer 模型、同一段 10.05 秒音频测试：CPU RTF 为 0.064，`provider=coreml` 为 0.065，并由 native runtime 明确打印 `Fallback to cpu!`。因此旧架构中的“Apple Silicon → CoreML → Qwen3-ASR”只是配置推断，并没有使用 GPU。

Apple GPU 路线应拆成独立适配器，首选 `whisper.cpp + Metal`，不要继续复用当前 sherpa provider 字段。M4 16GB 的默认模型建议为多语言 `Whisper large-v3-turbo` 的量化版本；它比 small 更适合作为中文、英文和专业术语混合场景的质量/速度平衡。`Whisper small` 只作为低内存、最低延迟档。现有 `Qwen3-ASR 0.6B int8` 保留给 CUDA 和 CPU 对比评估；在没有经过模型级 CoreML/Metal 基准前，不把它配置为 Apple GPU 默认模型。

Metal 适配器完成前，macOS 自动模式会诚实地视为“无可用本地 GPU 后端”，有完整阿里云凭证时使用云端；用户仍可显式选择本地 CPU。设置页同时展示“物理硬件”和“当前推理后端”，避免再次把 M4 GPU 与实际执行 provider 混为一谈。

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
| 规划：large-v3-turbo Q5_0 (Metal) | 待 M4 基准 | 待固定语料实测 |
| 新：阿里云云端 | 200–500ms（段结束后） | <5% |

> CER 数字为估算，以实际 AISHELL-1 bench 结果为准。
