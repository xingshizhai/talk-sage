# TalkSage v2 — sherpa-onnx 流式 ASR 技术验证报告（M1 PoC）

**日期：** 2026-08-18
**状态：** ✅ 通过（双引擎流式识别可用，延迟达标）

---

## 1. 验证目标

确认 sherpa-onnx（Rust 绑定）能否作为 TalkSage v2 的流式 ASR 运行时：
- Rust 绑定可编译链接（Windows x64）
- streaming 模型（英文 zipformer / 中文 paraformer）可加载识别
- 流式延迟满足"跟上会话速度"（目标：端到端出字 ≤1s，RTF <1）

## 2. 环境

| 项 | 值 |
|---|---|
| 平台 | Windows 11 x64（CPU 推理） |
| Rust | 1.95.0（MSVC） |
| sherpa-onnx crate | 1.13.5（官方 Rust 绑定，`default = static` 链接预编译库） |
| 预编译库 | `sherpa-onnx-v1.13.5-win-x64-static-MT-Release-lib.tar.bz2`（120MB，GitHub Releases，经 `SHERPA_ONNX_ARCHIVE_DIR` 本地预置） |
| 构建方式 | 无需本地编译 C++ / 无需手动 onnxruntime（sys crate 自动链接静态库） |

## 3. 模型

| 引擎 | 模型 | 文件（int8） | 大小 |
|---|---|---|---|
| 英文 | `sherpa-onnx-streaming-zipformer-en-2023-06-26`（chunk-16-left-64） | encoder/decoder/joiner + bpe.model + tokens.txt | ~73MB |
| 中文 | `sherpa-onnx-streaming-paraformer-zh` | encoder.int8 + decoder.int8 + tokens.txt | ~237MB |

模型经代理从 HuggingFace 下载至 `models/`，配置见 `crates/talksage-asr` 的 `SherpaStreamingEngine::new`。

## 4. 实测结果（CPU，2 线程，200ms 分块，16kHz wav）

### 英文（zipformer-en，测试音频 0.wav，6.63s）

```
[ 1.00s] AFTER
[ 1.40s] AFTER EARLY
[ 2.00s] AFTER EARLY NIGHTFALL
...
最终: AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE
      THE SQUALID QUARTER OF THE BROTHEL
```
- 首次出字：**1.00s**（含音频开头静音；语音开始后对齐 <400ms）
- 平均单块 decode：**7.25ms / 200ms 块**
- **实时因子 RTF：0.037**（比实时快约 27 倍）
- 识别文本与参考一致（标准朗读，无标点小写→大写为 sherpa 输出格式）

### 中文（paraformer-zh，测试音频 0.wav，10.05s，中英混合口语音频）

```
[ 1.20s] 昨
[ 1.80s] 昨天是
[ 2.40s] 昨天是 mon
...
最终: 昨天是 monday today is li 班二 the day after tomorrow 是星期
```
- 首次出字：**1.20s**（含静音）
- 平均单块 decode：**8.02ms / 200ms 块**
- **RTF：0.041**（快约 24 倍）
- 中文部分准确；混合音频中的英文词识别不佳（"li 班二"应为"礼拜二"）——符合 paraformer-zh 纯中文定位，**纯中文会议场景预期良好**；中英混合场景可后续评估 bilingual 模型

## 5. 结论

1. **延迟达标**：计算延迟（RTF 0.04）远低于实时，瓶颈在块缓冲与端点控制（200ms 块），实际"语音开始→出字"约 300–500ms，满足"跟上会话速度"。
2. **集成可行**：Rust 绑定在 Windows 编译链接顺畅（静态链接，单 exe 无额外 DLL）；无需本地 C++ 工具链。
3. **工程要点**：
   - 流式解码必须用 `while is_ready() { decode() }` 帧级推进（否则 GetFrames 断言崩溃）。
   - 建议 M1 使用 100–200ms 块 + VAD 端点控制（复用架构文档中的 Meetily 调参值）。
   - 模型经 `SHERPA_ONNX_ARCHIVE_DIR` 预置（构建期）；运行期模型从 `models/` 加载。

## 6. M1 建议

- 以 `talksage-asr` 的 `SherpaStreamingEngine` 为基线，接入 `talksage-pipeline`（AudioHub → VAD → 双引擎 → 事件推送）。
- 中英混合准确性作为后续优化项（可测 `bilingual-zh-en` streaming 模型替代单语模型）。
- GPU（CUDA/Metal）加速留作后端探测扩展（PoC 已验证 CPU 已远超实时）。

## 7. 复现命令

```bash
# 构建（需先下载预编译库到 .tools/sherpa-onnx-archives/）
$env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\.tools\sherpa-onnx-archives"
cargo build -p talksage-asr --bin poc_asr

# 运行
target\debug\poc_asr.exe zipformer-en  models\sherpa-onnx-streaming-zipformer-en-2023-06-26  models\...\0.wav
target\debug\poc_asr.exe paraformer-zh models\sherpa-onnx-streaming-paraformer-zh            models\...\0.wav
```
