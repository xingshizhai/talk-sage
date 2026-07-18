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
> - 模型缓存在系统用户目录，后续启动无需重新下载

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

  # 客户声音（英文）→ faster-whisper
  client:
    model: small          # tiny / base / small / medium / large-v3
    device: cpu           # cpu / cuda（有 GPU 时改为 cuda）
    compute_type: int8    # int8（CPU）/ float16（GPU）
    vad_filter: true

  # 用户声音（中文）→ FunASR Paraformer
  user:
    model: paraformer-zh
    device: cpu

  # 视频会议：捕获系统音频作为客户声源
  loopback:
    enabled: false        # 改为 true 启用
    device: null          # null = 自动检测；或填写设备编号（见下方）
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

首次启动会弹出**设置向导**（ASR / LLM / 可选知识库）。之后状态栏显示 **ASR 加载中…** → **ASR 就绪**。首次点击「▶ 开始监听」时会弹出录音同意确认。停止监听后可点「生成纪要」，结果追加写入当次会话 Markdown。

将 `setup.completed` 改回 `false` 可再次打开向导。

## 隐私与合规

### 录音同意

TalkSage 会采集麦克风音频，并在启用系统回环时采集扬声器输出。许多地区要求录音前征得参与者同意。首次开始监听前，应用会要求你确认已了解并承担合规义务。同意结果保存在 `~/.talksage/config.yaml` 的 `privacy.recording_consent_accepted`。

### 数据如何离开本机

- **音频**：默认在本地由 faster-whisper / FunASR 识别，不上传。
- **文本**：启用术语解释等插件时，转写后的文本会发送到你配置的 LLM API。
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
- [Post-MVP 进度与 Phase 3 计划](docs/superpowers/plans/2026-07-18-post-mvp-progress.md)
- [Phase 1 MVP 历史计划](docs/superpowers/plans/2026-06-18-talksage-phase1-mvp.md)

### 项目结构

```
talk-sage/
├── core/
│   ├── asr/
│   │   ├── base.py                 # ASREngine 抽象基类
│   │   ├── factory.py              # local / cloud 工厂
│   │   ├── faster_whisper_engine.py
│   │   ├── funasr_engine.py
│   │   ├── openai_cloud_engine.py  # 云端 Whisper API
│   │   └── dual_engine.py
│   ├── audio_hub.py                # 音频采集（麦克风 + 回环）
│   ├── echo_filter.py              # 双路串音抑制
│   ├── session_store.py            # 会话 Markdown 落盘
│   ├── conversation_state.py       # 话题/未决问题启发式跟踪
│   ├── knowledge_base.py           # 本地客户简报检索
│   ├── notes_generator.py          # 会后纪要生成
│   ├── pipeline.py                 # 主管线（音频 → ASR → 插件 → UI）
│   ├── plugin_bus.py               # 插件总线（支持骨架→终稿）
│   └── models.py                   # 数据模型
├── plugins/
│   ├── base.py                     # 插件抽象基类
│   ├── term_explainer.py           # 术语解释插件
│   └── brief_retriever.py          # 客户简报检索插件
├── llm/
│   ├── base.py                     # LLM Provider 接口
│   └── openai_compat.py            # OpenAI 兼容实现（DeepSeek/Kimi/Groq 等）
├── ui/
│   ├── main_window.py              # 主窗口
│   ├── setup_wizard.py             # 首次设置向导
│   ├── consent_dialog.py           # 录音同意弹窗
│   ├── screen_share.py             # 屏幕共享排除（Windows）
│   ├── style.py                    # 深色主题样式
│   └── sections/
│       ├── context.py              # 对话上下文
│       ├── transcript.py           # 实时转写区
│       ├── terms.py                # 术语卡片区
│       └── suggestions.py          # 简报提示区
├── config/
│   ├── manager.py                  # 配置管理
│   ├── defaults.yaml               # 内置默认值
│   └── config.template.yaml        # 用户配置模板
├── tests/                          # 单元测试
├── main.py                         # 程序入口
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

产品与交互上参考了 [OpenOats](https://github.com/yazinsai/OpenOats)（会议侧边栏转写、本地 ASR、知识库提示、屏享隐形、录音同意与会话落盘等思路）。TalkSage 定位不同：面向**中英双语商务/技术沟通**，侧重双引擎 ASR、术语解释与客户简报辅助，而非通用笔记检索型会议助手。
