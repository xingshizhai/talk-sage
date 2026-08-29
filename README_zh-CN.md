<p align="center">
  <strong>拓思者 · TalkSage</strong><br/>
  <em>你的个人 AI 会议助理 —— 转写、识人、分析、纪要，全在本机。</em>
</p>

<p align="center">
  <a href="#功能特性">功能特性</a> ·
  <a href="#快速开始">快速开始</a> ·
  <a href="#使用指南">使用指南</a> ·
  <a href="#技术架构">技术架构</a> ·
  <a href="#自动化测试">自动化测试</a> ·
  <a href="#文档">文档</a> ·
  <a href="README.md">English</a>
</p>

> **拓思者** —— TalkSage 的音译联想：Talk≈拓（开拓）、Sage≈思（思考）。把每一场会议，变成可复用、可追溯的结构化智慧。

**平台支持**：Windows（完整功能，含系统回环双流采集与 Vulkan GPU ASR）、macOS / Linux（麦克风单流；系统回环当前仅 Windows 支持，macOS 已声明麦克风权限 TCC）。

---

## 产品定位

拓思者采用**本地优先**架构：采集、录音、GPU ASR、说话人归属和媒体解码均在本机执行。只有用户明确选择阿里云回退或配置远程 OpenAI 兼容 LLM 时，音频或转写文本才会发送给对应服务商。系统提供实时转写、说话人归属、术语解释、翻译、要点聚合、知识库简报与会议纪要，并保留原始录音用于回归测试。

## 功能特性

- **实时 ASR**：VAD 分段、partial 即时上屏与智能句读；降噪默认关闭以保留远端弱语音（嘈杂环境可在设置页开启）
- **本地 GPU ASR**：Windows x64 使用 **whisper.cpp + Vulkan**（AMD / Intel / NVIDIA），macOS Apple Silicon 使用 **whisper.cpp + Metal**，均运行 Whisper large-v3-turbo Q5_0（约 547 MiB）。检测到受支持 GPU 时自动路由，否则回退阿里云或 CPU。
- **场景模式**：六套完整预设——**单人听写 / 一对一会话 / 双语对话 / 多人会议 / 演讲课堂 / 自定义**。一对一会话是默认模式，采用低开销的通道归属；仅多人会议默认启用 WeSpeaker 声纹聚类。
- **说话人归属**：明确的 `off / channel / voiceprint` 策略。`channel` 直接按麦克风/系统音频标记角色，不加载模型；`voiceprint` 匹配已登记主人并将其他说话人聚类为「客户1」「客户2」…
- **会议实时智能**：术语/缩写解释、中英实时翻译、本地规则要点聚合（问句/要求/决策/行动/技术，含数字与时间启发式）、知识库简报命中；历史详情可 **AI 提炼核心要点**（LLM，需配置）
- **会议纪要**：内置模板 + 任意 OpenAI 兼容 LLM（DeepSeek / Kimi / Ollama / Claude…）
- **录音与测试闭环**：每次监听保存原始分轨，历史页提供完整主录音回放（单流复用；双流立体声合并）；`talksage trim` 可准备回归素材
- **会议媒体实时会话**：WAV、MP3、MP4/M4A 在本地解码后，与麦克风会话共用场景 ASR 路由、逐句上屏、暂停/停止、术语、翻译、要点、录音和落库链路；可选 1×/2×/4×/极速，完成后停留在当前转写页。
- **会话质量评估**：自动判定会话为 正常/噪音/静音/待复核（阈值可配置 + 自动检测背景噪音校准）；噪音会话自动跳过要点聚合等下游分析
- **运行时噪音控制**：监听中从左侧面板实时调节噪音电平阈值（麦克风电平表 + 滑块），无需停止或重启
- **历史会话**：SQLite 存档，全文搜索、逐段时长/能量统计、质量徽章；**每次会话自动保存运行环境快照**（场景模式 / ASR 引擎 / VAD / 降噪 / 最短提交 / 增益 / 说话人模式 / 应用版本）——事后可对比不同模型与参数下的转写质量，或用 `talksage session replay <id>` 按相同/指定引擎再转写并另存新会话（历史详情与 `talksage session show <id>` 均可查看）
- **双载体**：Tauri 2 桌面应用（IPC）与 headless HTTP/WS 服务（浏览器访问，Token 鉴权）
- **系统托盘**：Windows 最小化即隐藏到右下角托盘（点击图标恢复）；macOS 遵循系统惯例，菜单栏常驻图标可快速显示/隐藏窗口
- **固定语料评测**：`talksage bench` 对 `*.wav` 语料逐个跑流式转写（引擎池热启动，进程内复用模型），输出 **CER/WER 准确率 + 实时率 RTF + 首词延迟**，供模型/参数回归对比（借鉴 WhisperLiveKit bench）
- **OpenAI 兼容转写 API**：headless 服务提供 `POST /v1/audio/transcriptions` 与 `GET /v1/models`，既有 OpenAI 生态客户端/脚本（whisper 类工具、curl、openai SDK）可直接指向本机做**本地转写**（Bearer 鉴权，json/text/verbose_json，任意采样率 wav 自动重采样）
- **噪音短段抑制**：`audio.min_segment_ms` 最短提交时长（设置页可调），final 段时长低于阈值的直接丢弃——噪音会话中偶发的"哒/咔"短段不再污染转写与历史
- **会中会话指标**：实时显示 我/客户发言占比、语速（WPM）、提问数、独白检测、打断计数与**会话健康分**（纯统计无 LLM，借鉴 Call.md conversation-metrics）
- **会中实时提示**：规则驱动 + 2 分钟限流的"coaching"提示（发言失衡/提问偏少/语速过快/临结束提醒下一步），浮动卡片可关闭（借鉴 Call.md nudge-engine）
- **三段式智能纪要**：并行生成 叙事概述 + **归属发言人的主题要点** + **行动项清单**（历史页"智能纪要"，借鉴 Call.md summary-generator）
- **会议结束 Webhook**：会话结束自动推送结构化数据（会议元数据 + 会话指标 + 质量 + 纪要 + 完整转写）到 n8n/Zapier/CRM，**调用前 SSRF 防护**（拒绝内网/回环地址，设置页可配置）
- **Markdown 结构化导出**：历史页一键导出单文件（概览/指标 → 纪要 → 智能纪要 → 转写），桌面端同时落盘 `<data_dir>/exports/`

## 快速开始

### 环境要求

- **Rust**（stable，Windows 需 MSVC 工具链 / macOS clang / Linux gcc）
- **Node.js 18+**（前端构建）
- **Python 3**（模型下载脚本，仅用标准库）
- Windows：**VS 2022 Build Tools**（含 C++ 工作负载，用于 Tauri 与 sherpa-onnx 静态链接）
- Windows x64 GPU ASR（可选，脚本自动检测）：**Vulkan SDK** 与 **LLVM**（`LIBCLANG_PATH` 指向 LLVM `bin`）

### 1. 下载模型

**方式 A：应用内安装（推荐）** — 打开左侧 **模型管理**，点「下载」即可；
安装/删除前需先停止监听，下载在后台进行并可看到进度条。模型管理支持磁盘空间预检、未完成下载续传、完整性校验和安全删除；正式安装时模型保存在用户数据目录，不写入 `.app`。

模型可用状态、目录解析、下载状态机、校验与日志规则见 [模型管理架构](docs/model-management.md)。

**方式 B：命令行脚本**（批量/离线环境）：

```bash
# 需要代理时：
# export https_proxy=http://127.0.0.1:10808 http_proxy=http://127.0.0.1:10808
python scripts/download_models.py all            # 当前产品模型与公共模型
python scripts/download_models.py qwen3-asr      # CUDA/CPU 模型
python scripts/download_models.py whisper-metal # Apple Metal 模型（可预下载）
python scripts/download_models.py legacy        # 仅测试需要的旧模型
```

默认高精度路径是 VAD + 段级本地 GPU ASR：Windows x64 通过 whisper.cpp/Vulkan 在 AMD / Intel / NVIDIA GPU 上运行 Whisper large-v3-turbo Q5_0，Apple Silicon 通过 whisper.cpp/Metal 运行同一模型，NVIDIA CUDA 路径可使用 Qwen3-ASR。没有可用 GPU 后端时回退阿里云实时 ASR（需完整凭证）或显式 CPU 诊断模式。

下载到 `models/`：

| 模型 | 用途 |
|---|---|
| `sherpa-onnx-qwen3-asr-0.6b` | Qwen3-ASR 0.6B 离线段级（int8） |
| `whisper.cpp-large-v3-turbo-q5_0` | Windows Vulkan + Apple Silicon Metal GPU ASR（约 547 MiB） |
| `silero-vad/silero_vad.onnx` | 语音活动检测（VAD） |
| `wespeaker/wespeaker_zh_cnceleb_resnet34.onnx` | 声纹模型（说话人识别） |

Paraformer、Zipformer 和 sherpa ONNX Whisper 已从产品模型列表移除；仓库中已有文件不会自动删除，以免破坏测试数据。如需清理，可在确认不再运行旧评测后手动删除对应目录。

### 2. 构建

**Windows**

```powershell
.\scripts\talksage.ps1 env      # 环境检查
.\scripts\talksage.ps1 build    # cargo + 前端（debug：CLI + 可独立运行的 debug App）
.\scripts\talksage.ps1 build --release  # cargo + 前端（release，不打包安装器）
.\scripts\talksage.ps1 run      # 运行桌面 debug 版；run --release 运行 release 版
```

`talksage.ps1 dev / build / package` 会自动配置 Vulkan SDK、`LIBCLANG_PATH`、Windows 短路径 `CARGO_TARGET_DIR` 与静态 CRT。非默认安装路径可复制 [`scripts/talksage.local.example.ps1`](scripts/talksage.local.example.ps1) 为 `scripts/talksage.local.ps1` 后覆盖。

**macOS / Linux**

```bash
./scripts/talksage.sh env       # 环境检查
./scripts/talksage.sh build     # cargo + 前端（debug：CLI + 可独立运行的 debug App）
./scripts/talksage.sh build --release  # cargo + 前端（release，不打包 dmg）
./scripts/talksage.sh run       # 运行桌面 debug 版；run --release 运行 release 版
./scripts/talksage.sh package   # 打包 dmg / 拓思者.app
```

完整手动步骤（sherpa 静态链接、代理说明、打包）见 [BUILDING.md](docs/BUILDING.md)。

### 3. 运行

```bash
# 桌面应用（默认 debug；加 --release 运行 release 版）
./scripts/talksage.ps1 run              # Windows（debug）
./scripts/talksage.ps1 run --release    # Windows（release）
./scripts/talksage.sh run               # macOS / Linux（debug）
./scripts/talksage.sh run --release     # macOS / Linux（release）

# CLI 实时转写（麦克风）
cargo run -p talksage-cli -- listen --input mic

# CLI 导入媒体（无需 GUI；支持 WAV / MP3 / MP4 / M4A）
talksage listen --input meeting.wav
talksage transcribe meeting.mp4 --save          # 提取音轨、转写并保存为新会话
talksage session replay 8                       # 用历史会话录音再转写，另存新会话

# headless Web 服务（浏览器访问 http://127.0.0.1:8080）
talksage serve --host 127.0.0.1 --port 8080

# 固定语料评测（bench-corpus/ 下放 *.wav + 同名 .txt 参考文本）
talksage bench --dir bench-corpus --engine paraformer-zh

# OpenAI 兼容转写（curl 直接调，等价 whisper API；token 由 TALKSAGE_SERVER_TOKEN 启用）
curl http://127.0.0.1:8080/v1/audio/transcriptions \
  -H "Authorization: Bearer $TALKSAGE_SERVER_TOKEN" \
  -F file=@meeting.wav -F model=paraformer-zh -F response_format=json
```

## 使用指南

| 操作 | 方式 |
|---|---|
| 开始监听 | 左侧 ▶ 开始监听（自动跳转实时转写页） |
| 导入会议媒体 | 实时转写页 → 导入录音文件（WAV / MP3 / MP4 / M4A） |
| 注册你的声音 | 设置 → 声音标识 → 录制我的声音（6 秒） |
| 麦克风电平 / 噪音调节 | 监听中左侧面板：麦克风电平表 + 噪音电平阈值滑块（实时生效，无需重启） |
| 录音去静音 | `talksage trim rec.wav [-o out.wav] [--preset sensitive\|standard\|strict]` |
| 纯录音 | `talksage record --seconds 60 [--input loopback]` |
| 离线转写 | `talksage transcribe audio.mp3`（加 `--save` 落库）；`talksage import audio.mp4` 是 `--save` 别名 |
| 环境诊断 | `talksage doctor` |
| 会话 | `talksage session list/show/search/rename/delete/export/notes/trio`；`talksage session <id>` 等同 show |
| 会话再转写 | `talksage session replay <id> [--engine qwen3-asr]`（用该会话录音另存新会话） |
| 模型 | `talksage models list/download/remove/gpu`（删除需 `--yes`） |
| 配置 | `talksage config path`；`config get [点路径]`；`config set <点路径> <值>`（密钥打码） |
| 日志 | `talksage logs [--lines 200]` |
| 离线说话人时间轴 | `talksage diarize audio.wav [--speakers N]`（pyannote 分段 + WeSpeaker 聚类） |
| 固定语料评测 | `talksage bench [--dir 语料目录] [--engine paraformer-zh\|zipformer-en] [--limit N]`（输出 CER/WER、RTF、首词延迟） |
| 噪音短段抑制 | 设置 → 音频处理 → 最短提交时长（ms，0=不限制）；或配置 `[audio] min_segment_ms` |

### 录音 → 裁剪 → 回放闭环

每次监听自动按流保存原始 wav 到 `<data_dir>/sessions/<id>/recordings/`（`talksage record` 仍写 `<data_dir>/recordings/`），可直接作为回归测试素材：

```powershell
.\scripts\recording_loop.ps1        # 裁剪全部录音 + 真实 ASR 回放
.\scripts\talksage.ps1 loop
```

详见 [docs/RECORDING.md](docs/RECORDING.md)。

## 技术架构

![拓思者架构图](docs/architecture.png)

Rust workspace 单二进制（无 Python 运行时），由单一应用服务和与传输无关的领域事件总线组成：

```
音频输入（麦克风 / Windows 回环 / WAV·MP3·MP4）→ 有界采集队列
        → 双流公平调度 → Preprocessor → VAD → ASR
        → 段生命周期 → 说话人归属 → EventFilter 链
        → DomainEvent → Tauri IPC / WebSocket / CLI
        ├── 有界 SessionWriter → SQLite + WAV 元数据
        └── 有界 PluginExecutor → observer 结果
停止 → writer barrier → 会话 finalizer（质量 / webhook）
```

`TalkSageService` 是桌面端、headless 服务、CLI 监听/导入和离线转写共用的唯一组装根。每条流独立拥有 VAD、ASR 引擎、采样时钟、端点状态、统计与说话人分配；SQLite 和 LLM 工作均离开音频热路径，慢消费者不会无限增长内存。

多人会议模式在 WeSpeaker 模型存在时启用在线声纹聚类。主人声音登记是可选的，匹配聚类标记为「我」；其他预设默认不执行声纹推理，除非自定义模式显式选择。

| Crate | 职责 |
|---|---|
| `talksage-core` | 领域事件、采样时钟、转写状态、说话人归属、会话指标 |
| `talksage-audio` | 麦克风/回环采集、媒体解码、重采样、降噪、WAV 读写、静音裁剪 |
| `talksage-asr` | ASR 适配器：sherpa-onnx 流式、whisper.cpp GPU（Vulkan / Metal）、阿里云 |
| `talksage-pipeline` | 共享服务、双流公平调度、段生命周期、有界插件/持久化 worker |
| `talksage-plugins` | 插件注册表：8 个内置插件（filter/observer/finalizer 三类钩子，短段抑制/跨流去重/指标/术语/翻译/简报/质量/Webhook） |
| `talksage-session` | SQLite 存储、兼容 schema 迁移与质量评估 |
| `talksage-notes` | 纪要模板 + 生成器 |
| `talksage-server` | axum headless 服务（REST + WS + SPA） |
| `talksage-cli` | 启动器：listen / transcribe / session / models / config / logs / serve / doctor |
| `web/` | Tauri 2 壳 + React/Vite/TS 界面 |

## 自动化测试

```bash
./scripts/talksage.sh test      # macOS/Linux：Rust + 前端 + 脚本测试
cargo test --workspace         # Rust 单元 + 真实模型测试（缺模型时明确跳过）
cd web && npm test             # 前端 68 项测试（当前套件）
```

Windows 使用 `.\scripts\run_tests.ps1` 或 `.\scripts\talksage.ps1 test`。真实模型集成测试覆盖中/英文 ASR、双流公平性与事件、录音落盘、结构化说话人归属、静音裁剪、服务端 API 与噪音会话质量；模型缺失时会显式提示跳过。

## 文档

- [architecture-v2.md](docs/architecture-v2.md) — 当前架构：共享服务、有界 worker、插件、持久化与采样时钟
- [plugin-development.md](docs/plugin-development.md) — 插件生命周期、开发指南、测试清单与机制评估
- [BUILDING.md](docs/BUILDING.md) — 编译与打包指南
- [vulkan-gpu-build.md](docs/vulkan-gpu-build.md) — Windows Vulkan GPU 编译、CRT 链接与排障
- [cli.md](docs/cli.md) — CLI：会话 / 转写 / 模型 / 配置 / 日志
- [RECORDING.md](docs/RECORDING.md) — 录音/裁剪/回归闭环
- [LOGGING.md](docs/LOGGING.md) — 结构化日志与排障
- [testing.md](docs/testing.md) — 自动化测试策略
- [real-time-transcription.md](docs/real-time-transcription.md) — 实时转写行为、计时、显示模式与改进路线
- [evaluation-user-guide.md](docs/evaluation-user-guide.md) — 语音自动化测试与模型评估使用手册
- [evaluation-framework.md](docs/evaluation-framework.md) — 评估框架架构与指标设计
- [terminology.md](docs/terminology.md) — 专业术语热词、纠错表与术语指标

## 仓库结构

```
crates/            Rust workspace 领域 crate
web/               Tauri 2 + React 前端
scripts/           构建/运行/测试工具 + 模型下载器
  talksage.ps1              Windows 一体化脚本（自动配置 Vulkan 环境）
  talksage.local.example.ps1  本机路径覆盖模板（复制为 talksage.local.ps1）
  build-vulkan.bat          独立 Vulkan GPU 编译脚本
  talksage.sh               macOS/Linux 对等脚本
vendor/            fork crate（whisper-rs-sys：Vulkan 静态 CRT 补丁）
docs/              设计与运维文档
models/            运行期模型（gitignore，约 1.2GB，多引擎可选）
```

## 许可证

[GNU 通用公共许可证第 3 版（GPLv3）](LICENSE)
