# TalkSage

实时 AI 会议助手——帮助中文用户在与英文客户的技术方案讨论、商务谈判时，自动识别语音、解释术语、提供策略建议。

## 功能

- **双引擎本地语音识别**：英文（客户）→ faster-whisper，中文（用户）→ FunASR Paraformer；也可切换 `transcribe.mode: cloud`
- **实时术语解释**：识别英文缩写后先显示骨架，再流式填充 LLM 解释（去重 + 冷却）
- **双音频流**：同时捕获麦克风（用户）和系统回环（客户）；自动抑制回声串音
- **会话自动保存**：监听期间写入 `~/.talksage/sessions/*.md`，可一键生成会议纪要
- **对话上下文**：自动跟踪话题、未决问题与近期决策
- **客户简报知识库**：可选本地 `.md/.txt` 文件夹，会议中按关键词检索提示
- **首次设置向导**：引导选择 ASR 模式、LLM 与知识库
- **设备与音频优化**：设置页选择麦克风/回环；GPU 自动探测；麦克风闪避与防削波
- **导入音频**：支持导入录音离线转写并保存到会话目录
- **SQLite 会话历史**：转写可检索；「历史」按钮浏览/搜索
- **可选 Parakeet 英文 ASR**：设置中切换（需 `pip install "onnx-asr[cpu,hub]"`）
- **可选 BitNet CPU 英文 ASR**：经 [VibeASR.cpp](https://github.com/microsoft/VibeASR.cpp) 本地推理；导入音频默认优先使用
- **插件化架构**：翻译、技术评估、谈判分析等功能可按需启用
- **多 LLM 支持**：DeepSeek、Kimi、MiniMax、Groq、Ollama、Claude

## 系统要求

- Python 3.11+
- Windows 10/11 / macOS 12+ / Ubuntu 20.04+
- 内存：建议 8GB+（运行双 ASR 模型时）
- GPU（可选）：CUDA 11.8+ 可显著降低延迟

## 安装

### 1. 克隆项目

```bash
git clone <repo-url>
cd talk-sage
```

### 2. 创建虚拟环境

```bash
python -m venv .venv

# Windows
.venv\Scripts\activate

# macOS / Linux
source .venv/bin/activate
```

### 3. 安装依赖

```bash
pip install -r requirements.txt
```

> **首次运行说明**
> - faster-whisper 会在第一次使用时自动下载所选 Whisper 模型（`small` 约 244MB）
> - FunASR 会在第一次使用时自动从 ModelScope 下载 `paraformer-zh` 等模型（共约 700MB）
> - 若启用 Parakeet，首次还会通过 onnx-asr 下载对应 ONNX 模型
> - `device: auto` 会在启动时探测 GPU；模型缓存在系统用户目录，后续无需重下

### 4. 系统音频依赖（Linux）

Linux 需要额外安装 PortAudio：

```bash
# Ubuntu / Debian
sudo apt install portaudio19-dev

# Fedora
sudo dnf install portaudio-devel
```

macOS 可通过 Homebrew 安装：

```bash
brew install portaudio
```

## 配置

将配置模板复制到用户目录：

```bash
# Windows
copy config\config.template.yaml %USERPROFILE%\.talksage\config.yaml

# macOS / Linux
cp config/config.template.yaml ~/.talksage/config.yaml
```

然后编辑 `~/.talksage/config.yaml`，填入所需的 LLM API Key：

```yaml
llm:
  providers:
    deepseek:
      api_key: "你的 DeepSeek API Key"   # 推荐，中文理解好
```

### ASR 引擎配置

```yaml
transcribe:
  mode: local             # local | cloud
  # mode: cloud 时使用（OpenAI 兼容 Whisper API）
  # cloud:
  #   api_key: "sk-..."
  #   base_url: https://api.openai.com/v1
  #   model: whisper-1

  # 客户声音（英文）
  client:
    engine: faster-whisper  # faster-whisper | parakeet | bitnet
    model: small            # whisper: tiny/base/small/...；parakeet: nemo-parakeet-tdt-0.6b-v3
    device: auto            # auto / cpu / cuda（auto 启动时探测 GPU）
    compute_type: auto      # auto / int8 / float16（仅 faster-whisper）
    vad_filter: true

  # 用户声音（中文）→ FunASR Paraformer
  user:
    model: paraformer-zh
    device: auto

  # 视频会议：捕获系统音频作为客户声源
  loopback:
    enabled: false        # 改为 true 启用
    device: null          # null = 自动检测；或填写设备编号（见下方）

  # BitNet CPU（可选，见下方「BitNet 安装」）
  bitnet:
    binary: ""            # asr_infer 路径；空则自动查找
    vae_model: ""
    lm_model: ""
    threads: 4
  import:
    prefer_bitnet: true   # 导入音频优先 BitNet
```

使用 Parakeet 时需额外安装：

```bash
pip install "onnx-asr[cpu,hub]"
```

#### BitNet 安装（可选）

1. 编译 [VibeASR.cpp](https://github.com/microsoft/VibeASR.cpp) 得到 `asr_infer`（Windows 需 MinGW，勿用 MSVC）
2. 从 Hugging Face 下载 [VibeVoice-ASR-BitNet](https://huggingface.co/microsoft/VibeVoice-ASR-BitNet) 的两个 GGUF
3. 任选一种配置方式：
   - 写入 `~/.talksage/config.yaml` 的 `transcribe.bitnet.binary / vae_model / lm_model`
   - 或将 `asr_infer` 与两个 `.gguf` 放到 `~/.talksage/vibeasr/`
   - 或设置环境变量 `TALKSAGE_VIBEASR_ROOT` 指向含二进制与模型的目录

设置页可将英文引擎切到「BitNet CPU」。导入音频在 `prefer_bitnet: true` 且路径可用时**整段**走 BitNet。

也可在应用内「设置」中切换英文引擎与设备，无需手改配置文件。

### 音频与会话

```yaml
audio:
  mic_device: null          # null = 系统默认；或 sounddevice 设备编号
  ducking:
    enabled: true           # 回环响时压低麦克风，减轻串音
  soft_limit:
    enabled: true           # 防削波
  crosstalk:
    similarity_threshold: 0.6

session:
  auto_save: true           # ~/.talksage/sessions/*.md
  sqlite: true              # ~/.talksage/sessions.db（「历史」可搜索）
```

#### 查看可用音频设备

```bash
python -c "import sounddevice; print(sounddevice.query_devices())"
```

Windows 上通常可以启用"立体声混音（Stereo Mix）"或使用 WASAPI Loopback；macOS 需要安装 BlackHole 虚拟声卡。

### 模型大小参考

| 模型 | 大小 | 推荐场景 |
|------|------|---------|
| `tiny` | 75MB | 快速测试，准确率低 |
| `base` | 145MB | 轻量场景 |
| `small` | 244MB | **CPU 日常推荐** |
| `medium` | 769MB | 准确率更高，需要更多内存 |
| `large-v3` | 1.5GB | 最高准确率，需要 GPU |

## 运行

```bash
python main.py
```

首次启动会弹出**设置向导**（ASR / LLM / 可选知识库）。之后状态栏显示 **ASR 加载中…** → **ASR 就绪**。首次点击「开始监听」时会弹出录音同意确认。

常用按钮：

| 按钮 | 作用 |
|------|------|
| 开始监听 / 停止 | 实时双路转写 |
| 生成纪要 | 会后 LLM 纪要，追加到当次会话 Markdown |
| 导入音频 | 离线转写录音并保存到会话目录 |
| 历史 | 浏览 / 搜索 SQLite 会话 |
| 设置 | 麦克风、回环、英文 ASR 引擎、GPU、闪避等 |

将 `setup.completed` 改回 `false` 可再次打开向导。

## 隐私与合规

### 录音同意

TalkSage 会采集麦克风音频，并在启用系统回环时采集扬声器输出。许多地区要求录音前征得参与者同意。首次开始监听前，应用会要求你确认已了解并承担合规义务。同意结果保存在 `~/.talksage/config.yaml` 的 `privacy.recording_consent_accepted`。

### 数据如何离开本机

- **音频**：默认在本地由 faster-whisper（或 Parakeet）/ FunASR 识别，不上传。
- **文本**：启用术语解释等插件时，转写后的文本会发送到你配置的 LLM API。
- **会话**：Markdown 与可选 SQLite 仅保存在本机 `~/.talksage/`。
- **API Key**：仅保存在本机配置文件中。

### 屏幕共享隐形

会议中建议对方看不到 TalkSage 侧边栏：

| 平台 | 行为 |
|------|------|
| **Windows 10 2004+** | 默认启用 `privacy.hide_from_screen_share`，通过系统 API 尽量排除本窗口被屏幕捕获 |
| **macOS / Linux** | 无统一系统 API；请在 Zoom/Teams 中选择「共享特定窗口」，不要共享整个桌面或 TalkSage 窗口 |

可将配置改为：

```yaml
privacy:
  hide_from_screen_share: false
```

## 开发

### 运行测试

```bash
pytest
```

### 设计文档

- [产品设计（含架构与完成度）](docs/superpowers/specs/2026-06-18-talksage-design.md)
- [BitNet ASR 接入设计](docs/superpowers/specs/2026-08-04-bitnet-asr-design.md)
- [Post-MVP 进度与 Phase 3 计划](docs/superpowers/plans/2026-07-18-post-mvp-progress.md)
- [Phase 1 MVP 历史计划](docs/superpowers/plans/2026-06-18-talksage-phase1-mvp.md)

### 项目结构

```
talk-sage/
├── core/
│   ├── asr/
│   │   ├── base.py / factory.py / dual_engine.py
│   │   ├── faster_whisper_engine.py / funasr_engine.py
│   │   ├── openai_cloud_engine.py / parakeet_engine.py / bitnet_engine.py
│   ├── audio_hub.py / audio_process.py / device_probe.py
│   ├── echo_filter.py / pipeline.py / plugin_bus.py
│   ├── session_store.py / session_db.py / import_audio.py
│   ├── conversation_state.py / knowledge_base.py / notes_generator.py
│   └── models.py
├── plugins/          # term_explainer, brief_retriever
├── llm/              # OpenAI 兼容 Provider
├── ui/
│   ├── main_window.py / setup_wizard.py / settings_dialog.py
│   ├── history_dialog.py / consent_dialog.py / screen_share.py
│   └── sections/     # context, transcript, terms, suggestions
├── config/
├── tests/
├── main.py
└── requirements.txt
```

## 支持的 LLM 提供商

| 提供商 | 推荐用途 |
|--------|---------|
| DeepSeek | 术语解释、翻译（中文理解强） |
| Kimi | 长上下文分析 |
| MiniMax | 商务建议 |
| Groq | 极速响应（术语解释） |
| Claude | 技术评估、谈判分析 |
| Ollama | 完全本地离线 |

## 参考项目

| 项目 | 借鉴点 | TalkSage 差异 |
|------|--------|---------------|
| [OpenOats](https://github.com/yazinsai/OpenOats) | 侧边栏转写、门控节流、屏享隐形、录音同意、会话落盘 | 非通用笔记 RAG；主线为中英术语 / 简报 / 谈判 |
| [Meetily](https://github.com/Zackriya-Solutions/meetily) | 本地 ASR 性能（Parakeet）、闪避/混音、SQLite、导入重转写 | 不做通用纪要工具；优先会中侧边栏辅助 |
| [VibeVoice / VibeASR.cpp](https://github.com/microsoft/VibeASR.cpp) | BitNet CPU 量化 ASR、长音频单次推理 | 作可选英文后端 + 导入优先；中文仍 FunASR |
