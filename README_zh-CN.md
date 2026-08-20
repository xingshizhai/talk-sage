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

**平台支持**：Windows（完整功能，含回环双流采集）、macOS / Linux（麦克风单流；系统回环采集客户语音当前仅 Windows 支持，macOS 已声明麦克风权限 TCC）。

---

## 产品定位

拓思者**完全本地运行**（音频不出本机、无云依赖）：通过麦克风（叠加系统回环采集远程通话）监听会议，获得**实时双语转写**、**说话人识别**（注册你的声音为主人，其余说话人自动区分）、**术语解释**、**实时翻译**、**要点聚合**、**知识库简报**与**会议纪要**，同时**自动保存原始录音**用于后续回归测试。适合销售拜访、跨语言沟通、技术评审等"边用边录、录完即测"的场景。

## 功能特性

- **实时流式转写**：中文 paraformer + 英文 zipformer 双流并行，增量 partial 即时上屏，silero VAD 自动分段；**按句分行**（句末标点/弱边界/长度软断）提升可读性；**降噪默认关闭**以保留远端弱语音识别（嘈杂环境可在设置页开启）
- **场景模式**：**生活 / 会议 / 会谈 / 自定义** 四种场景一键切换——生活（灵敏 VAD 抓短句弱语音、单流、关分析插件）、会议（双流 + 插件全开，默认）、会谈（双流 + 300ms 短段提交）；自定义可逐项编辑 VAD/降噪/最短提交/引擎/插件/说话人/噪音检测（设置页「场景模式」）
- **多人说话人识别**：设置页「声音标识」录制你的声音（6 秒），监听时先匹配主人（标记「我」），其余说话人自动区分为「客户1」「客户2」…（在线聚类，同人复用标签）
- **会议实时智能**：术语/缩写解释、中英实时翻译、本地规则要点聚合（问句/要求/决策/行动/技术，含数字与时间启发式）、知识库简报命中；历史详情可 **AI 提炼核心要点**（LLM，需配置）
- **会议纪要**：内置模板 + 任意 OpenAI 兼容 LLM（DeepSeek / Kimi / Ollama / Claude…）
- **录音与测试闭环**：每次监听按流保存原始 wav；`talksage trim` 用同一套 VAD 去掉静音；`scripts/recording_loop.ps1` 一键裁剪 + 回放验证
- **会话质量评估**：自动判定会话为 正常/噪音/静音/待复核（阈值可配置 + 自动检测背景噪音校准）；噪音会话自动跳过要点聚合等下游分析
- **运行时噪音控制**：监听中从左侧面板实时调节噪音电平阈值（麦克风电平表 + 滑块），无需停止或重启
- **历史会话**：SQLite 存档，全文搜索、逐段时长/能量统计、质量徽章
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

### 1. 下载模型（约 340MB）

```bash
# 需要代理时：
# export https_proxy=http://127.0.0.1:10808 http_proxy=http://127.0.0.1:10808
python scripts/download_models.py all
```

下载到 `models/`：

| 模型 | 用途 |
|---|---|
| `sherpa-onnx-streaming-paraformer-zh` | 中文流式 ASR |
| `sherpa-onnx-streaming-zipformer-en-2023-06-26` | 英文流式 ASR |
| `silero-vad/silero_vad.onnx` | 语音活动检测（VAD） |
| `wespeaker/wespeaker_zh_cnceleb_resnet34.onnx` | 声纹模型（说话人识别） |

### 2. 构建

**Windows**

```powershell
.\scripts\talksage.ps1 env      # 环境检查
.\scripts\talksage.ps1 build    # cargo + 前端（debug CLI）
# 桌面 release：
cd web
npx tauri build --no-bundle
```

**macOS / Linux**

```bash
./scripts/talksage.sh build
```

完整手动步骤（sherpa 静态链接、代理说明、打包）见 [BUILDING.md](docs/BUILDING.md)。

### 3. 运行

```bash
# 桌面应用（release）
./scripts/talksage.ps1 run          # Windows

# CLI 实时转写（麦克风）
cargo run -p talksage-cli -- listen --input mic

# CLI 回放 wav（无需 GUI）
talksage listen --input meeting.wav

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
| 注册你的声音 | 设置 → 声音标识 → 录制我的声音（6 秒） |
| 麦克风电平 / 噪音调节 | 监听中左侧面板：麦克风电平表 + 噪音电平阈值滑块（实时生效，无需重启） |
| 录音去静音 | `talksage trim rec.wav [-o out.wav] [--preset sensitive\|standard\|strict]` |
| 纯录音 | `talksage record --seconds 60 [--input loopback]` |
| 离线导入 | `talksage import audio.wav` |
| 环境诊断 | `talksage doctor` |
| 固定语料评测 | `talksage bench [--dir 语料目录] [--engine paraformer-zh\|zipformer-en] [--limit N]`（输出 CER/WER、RTF、首词延迟） |
| 噪音短段抑制 | 设置 → ASR 转写 → 最短提交时长（ms，0=不限制）；或配置 `[audio] min_segment_ms` |

### 录音 → 裁剪 → 回放闭环

每次监听自动按流保存原始 wav 到 `<data_dir>/recordings/`，可直接作为回归测试素材：

```powershell
.\scripts\recording_loop.ps1        # 裁剪全部录音 + 真实 ASR 回放
.\scripts\talksage.ps1 loop
```

详见 [docs/RECORDING.md](docs/RECORDING.md)。

## 技术架构

![拓思者架构图](docs/architecture.png)

Rust workspace 单二进制（无 Python 运行时），所有载体共享一套领域事件总线：

```
AudioHub（cpal / WASAPI 回环）→ Preprocessor（降噪/高通/噪声门）
        → VAD（silero）→ 流式 ASR（sherpa-onnx）→ final 段
        → 说话人识别（wespeaker）→ 插件（术语/翻译/简报/要点）
        → DomainEvent（serde）→ Tauri IPC 或 WS → React UI
        → 会话 SQLite（段 + 统计 + 质量 meta）
```

| Crate | 职责 |
|---|---|
| `talksage-core` | 领域事件、会话质量、文本噪音评分 |
| `talksage-audio` | 麦克风/回环采集、重采样、降噪、wav 读写、静音裁剪 |
| `talksage-asr` | sherpa-onnx 流式引擎封装 |
| `talksage-pipeline` | VAD 分段、双流、录音、运行时噪音电平阈值、说话人识别 |
| `talksage-plugins` | 术语解释 / 翻译 / 简报检索 |
| `talksage-session` | SQLite 存储 + 质量评估 |
| `talksage-notes` | 纪要模板 + 生成器 |
| `talksage-server` | axum headless 服务（REST + WS + SPA） |
| `talksage-cli` | 启动器：listen / trim / record / import / serve / doctor / bench |
| `web/` | Tauri 2 壳 + React/Vite/TS 界面 |

## 自动化测试

```bash
.\scripts\run_tests.ps1        # cargo test（单元 + 真实模型集成）+ vitest
cargo test --workspace         # Rust 全量（模型缺失自动跳过并提示）
cd web && npx vitest run       # 前端 27 用例
```

真实模型集成测试覆盖：中/英文 ASR 识别、双流事件、录音落盘、**说话人识别（主人 vs 新说话人）**、静音裁剪、服务端 API，以及"13:57 噪音会话质量判定"真实案例。

## 文档

- [architecture-v2.md](docs/architecture-v2.md) — v2 设计：双载体、延迟预算、快慢路径
- [BUILDING.md](docs/BUILDING.md) — 编译与打包指南
- [RECORDING.md](docs/RECORDING.md) — 录音/裁剪/回归闭环
- [LOGGING.md](docs/LOGGING.md) — 结构化日志与排障
- [testing.md](docs/testing.md) — 自动化测试策略
- [reference-whisperlivekit.md](docs/reference-whisperlivekit.md) — 参考项目：WhisperLiveKit 研究（引擎池/评测/双载体借鉴）
- [reference-callmd.md](docs/reference-callmd.md) — 参考项目：Call.md 研究（会话指标/实时提示/三段式纪要/Webhook 借鉴）

## 仓库结构

```
crates/            Rust workspace（10 个领域 crate）
web/               Tauri 2 + React 前端
scripts/           构建/运行/测试工具 + 模型下载器
docs/              设计与运维文档
models/            运行期模型（gitignore，约 340MB）
```

## 许可证

MIT
