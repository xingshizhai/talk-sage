# TalkSage v2 架构设计（推翻重设计）

**日期：** 2026-08（初版）
**状态：** 设计定稿，M0/M1 已实施
**旧版对照：** 旧 Python/PySide6 实现（v1）已随 v2 重写从仓库移除（git 历史可查）

---

## 1. 背景与目标

TalkSage 是一款**实时个人 AI 会议助理**：在视频会议或面对面洽谈中，听对方讲话，实时提炼关键内容、术语解释、关联知识/简报，帮助用户及时回应。旧版为 Python + PySide6 桌面应用（本地双引擎 ASR + 术语插件 + 会话落库），存在以下限制，决定**推翻重设计**：

1. **固定 3 秒分块转写**，延迟高、长句被切断，无法"跟上会话速度"
2. PySide6 UI 在视频会议场景下常驻体验一般，且无翻译/说话人分离/录音回放
3. Python 生态做本地实时音频 + ASR 推理性能与分发（打包）成本高
4. 无多设备/团队使用能力

### v2 目标

- **场景不变**：实时会议个人助理（视频会议回环 + 面对面洽谈双形态）
- **低延迟**：流式 ASR，端到端"跟上会话速度"，快路径全本地
- **形态**：Tauri 2 桌面应用为主（体验最优），核心域服务化预留（未来多设备/团队）
- **技术栈**：Rust 全栈核心 + React（Web 技术栈）UI，不再使用 Python
- **结构参考**：DeepSeek Harness（DSH）的工程结构思想（CLI launcher、能力按域拆包、宿主/客户端分离、配置分层、同一 UI 多载体）

---

## 2. 需求澄清结论（2026-08 确认）

| 维度 | 结论 | 架构影响 |
|---|---|---|
| 定位 | 实时个人助理：听对方讲话 → 关键点 + 关联知识 → 支撑及时回复 | 延迟是核心指标 |
| 部署 | 先单机；**架构预留团队/多设备** | 传输层可插拔、workspace/auth 抽象 |
| 会议形态 | 视频会议（系统回环）+ 面对面（纯麦）都要 | 双路链路都要稳，回环可缺省 |
| 实时性 | 越快越好，至少跟上会话速度 | **流式 ASR 必选**，非块式 |
| 平台 | Windows + macOS（macOS 支持 Apple GPU） | 回环双平台（WASAPI / ScreenCaptureKit） |
| 功能 | 术语解释、简报检索、**实时翻译**、**说话人分离**、**录音回放**、**纪要模板化** | pipeline 与事件模型扩展 |
| 硬件 | CPU 独立运行；有 GPU 优先（NVIDIA CUDA + Apple Metal） | 推理后端自动探测 |
| 界面 | **Tauri 2 原生壳 + React UI**；headless 服务模式预留 | 一套 UI 两种载体 |

---

## 3. 设计原则

1. **快慢路径分离**：本地规则/检索走快路径（<300ms），LLM 走慢路径异步填充，UI 一律"先骨架后填充"
2. **领域与传输解耦**：核心域（audio/asr/pipeline/session…）为纯 Rust 库，不依赖任何传输层；IPC 与 HTTP/WS 都是可插拔适配器
3. **事件驱动**：领域事件（Segment/Term/Translation/State/Status…）定义为与传输无关的纯数据结构，IPC 与 WS 传同一结构
4. **能力按域拆包**：参考 DSH 的 `packages/dsh-*` 模式，Rust workspace 每个能力一个 crate，可独立演进与测试
5. **默认回环安全**：服务化模式下默认绑定 127.0.0.1（DSH 同款安全姿态），对外开放需显式配置 + 鉴权
6. **隐私默认本地**：音频、转写、会话默认全部本地处理与存储；录音同意保留

---

## 4. 总体架构

```
┌─ 载体 1（默认）：Tauri 2 原生壳 ──────────────────────────────┐
│                                                                │
│   React UI（web/，Vite + React + TS）                          │
│      ▲ 统一 API 抽象（lib/api.ts：IPC 适配器）                 │
│      │                                                         │
│   Tauri IPC（command + event）                                 │
│      │                                                         │
│   Rust 核心域（crates/*，同进程）                               │
│     audio（采集/混音/VAD/录音） │ asr（streaming 双引擎）        │
│     pipeline（快慢路径） │ plugins │ session │ llm │ knowledge  │
│                                                                │
│   原生能力：托盘、窗口置顶/透明、屏享隐形、回环直采（壳内）       │
└────────────────────────────────────────────────────────────────┘
                          ▲ 同一套 UI 与核心域
                          │ 传输层可插拔
┌─ 载体 2（预留）：headless 服务 + 浏览器 ───────────────────────┐
│   React UI（同一套）                                           │
│      ▲ HTTP /api + WebSocket /ws                              │
│   talksage-server（axum，默认 127.0.0.1）                      │
│      ▲                                                        │
│   Rust 核心域（同一套 crates/*，独立进程）                      │
│   capture-agent（可选原生进程，补系统回环采集）                  │
└────────────────────────────────────────────────────────────────┘
```

**关键决策**：核心域是"库"，载体是"壳"。Tauri 模式下 IPC 直连（少一跳、回环壳内采集）；headless 模式下 HTTP/WS 暴露（支持手机/平板浏览器访问、团队部署，回环由可选 capture-agent 补上）。此双载体设计与 DSH 的 webserver 文档描述一致——"same Web UI can run in a browser or in an Electron shell carrying fetch over an IPC bridge"。

---

## 5. 技术选型

| 层 | 选型 | 理由 |
|---|---|---|
| 应用壳 | **Tauri 2**（Rust） | 原生窗口控制 + 系统集成 + Web 技术栈 UI；Meetily 同场景已验证 |
| 后端语言 | **Rust**（workspace） | 音频原生、ASR 原生绑定、单二进制、并发强 |
| Web 框架 | **axum**（仅 headless 模式） | tokio 生态，WebSocket/静态托管一体 |
| ASR 运行时 | **sherpa-onnx**（k2-fsa） | C API + Rust 绑定；支持 streaming paraformer（中文）、streaming zipformer/whisper（英文）、SenseVoice（导入/重转写高质量后端）、说话人 embedding；CPU/CUDA/CoreML 多后端 |
| 前端 | **Vite + React + TypeScript** | 流式列表/虚拟化/分区表达力强，HMR 快 |
| 音频采集 | cpal / 平台 API（WASAPI、ScreenCaptureKit） | 与 Meetily 同栈 |
| VAD | sherpa-onnx 内置 VAD（或 silero-vad Rust 绑定） | 流式端点检测 |
| 存储 | **SQLite**（rusqlite/sqlx）+ Markdown 导出 | 轻量、单文件、可检索 |
| LLM | OpenAI 兼容 HTTP 客户端（DeepSeek/Kimi/Groq/Ollama…） | 与旧版一致，抽象 Provider |
| 配置 | TOML（内置默认 + 用户文件 + 环境变量） | 分层合并（简化版 DSH patch 思想） |

---

## 6. 模块划分（Rust workspace + web/）

```
talksage/
├── crates/
│   ├── talksage-cli/          # launcher：web / serve / import / record / doctor
│   ├── talksage-core/         # 领域模型与事件类型（与传输无关）
│   ├── talksage-audio/        # AudioHub：采集抽象、流式 VAD、混音/闪避/限幅、录音器
│   ├── talksage-asr/          # sherpa-onnx 封装：streaming 双引擎、说话人 embedding、GPU 探测
│   ├── talksage-pipeline/     # 编排：快路径（ASR→缩写→检索→要点）+ 慢路径（LLM）
│   ├── talksage-plugins/      # term_explainer / brief_retriever / translator / notes
│   ├── talksage-session/      # SQLite + Markdown 导出 + 录音文件索引 + 历史检索
│   ├── talksage-llm/          # OpenAI 兼容 Provider 抽象 + 流式
│   ├── talksage-knowledge/    # 简报知识库检索（Jaccard，预留向量）
│   ├── talksage-import/       # 导入 + 重转写 + 离线 diarization
│   ├── talksage-config/       # 分层配置
│   ├── talksage-server/       # （可选）axum 适配器：REST + WS + 静态托管 + auth 抽象
│   └── talksage-tauri/        # （可选）Tauri 适配器：command/event 桥接
├── capture-agent/             # （可选）双平台回环采集进程（WASAPI / ScreenCaptureKit）
├── web/                       # React SPA
│   ├── src/pages/             # 会议页 / 历史页 / 设置页 / 向导
│   ├── src/sections/          # 上下文 / 转写 / 术语 / 翻译 / 要点 / 简报
│   ├── src/lib/api.ts         # 统一 API 抽象（IPC 适配器 ↔ HTTP 适配器）
│   └── src/lib/ws.ts          # 事件订阅（IPC 事件 ↔ WS 消息）
├── models/                    # streaming 模型清单与下载脚本
├── docs/
└── talksage.toml              # 用户配置
```

**参考映射（旧版 → v2）**：

| 旧（Python/PySide6） | v2 |
|---|---|
| core/audio_hub.py, audio_process.py, echo_filter.py | talksage-audio |
| core/asr/*（faster-whisper/funasr/bitnet） | talksage-asr（sherpa-onnx streaming 替换） |
| core/plugin_bus.py, models.py, conversation_state.py | talksage-core + talksage-pipeline + talksage-plugins |
| core/session_store.py, session_db.py | talksage-session |
| core/knowledge_base.py, notes_generator.py, import_audio.py | talksage-knowledge / talksage-plugins(notes) / talksage-import |
| llm/openai_compat.py | talksage-llm |
| ui/*（PySide6） | web/（React） |
| main.py, config/manager.py | talksage-cli + talksage-config |

---

## 7. 核心数据流与延迟预算

### 7.1 实时链路（Tauri 模式，默认）

```
麦克风（壳内 cpal） ──┐
系统回环（壳内 WASAPI/SCK） ──┤→ AudioHub（混音/闪避/限幅）
                          │
                          ▼
                 流式 VAD 端点检测（sherpa-onnx VAD）
                          │  语音段（增量流）
                          ▼
        DualStreamingASR：client→zipformer-en / user→paraformer-zh
                          │  增量文本（<500ms 端到端）
                          ▼
   ┌── 快路径（全本地）──────────────┐
   │ 缩写检测（规则）→ 术语骨架        │
   │ 关键词 → 简报检索（Jaccard）     │
   │ 关键短语/问句/决策（本地规则）    │
   │ 说话人 embedding 聚类（可选）    │
   └──────────┬───────────────────┘
              ▼ 领域事件（IPC 推送）
          React UI 即时渲染（先骨架）
              │
   ┌── 慢路径（LLM 异步）────────────┐
   │ 术语解释终稿（冷却+去重）        │
   │ 实时翻译（流式输出）             │
   │ 要点提炼 / 纪要（模板化）        │
   └──────────┬───────────────────┘
              ▼ 领域事件（填充更新，按 result_id 原地更新）
```

### 7.2 延迟预算（快路径目标）

| 阶段 | 预算 |
|---|---|
| 采集 → VAD 端点判定 | ≤ 200ms |
| ASR 增量出字（流式） | ≤ 500ms（端到端） |
| 缩写检测 / 简报检索 | ≤ 50ms |
| 术语骨架上屏 | ≤ 100ms（ASR 文本到达后） |
| LLM 慢路径填充 | 秒级（异步，不阻塞快路径） |

> 参考 Meetily 的 VAD 调参经验（`audio/vad.rs`）：redemption 2s、pre-speech pad 300ms、post-speech pad 400ms、min_speech 250ms、阈值 0.50/0.35，用于平衡"连续长句不切碎"与"静音不送 ASR"。

---

## 8. 关键子系统设计

### 8.1 音频域（talksage-audio）

- **采集抽象**：`CaptureSource` trait（麦克风 / 系统回环 / 文件），平台实现放 `platform/`（参考 DSH 按平台分文件、Meetily `audio/devices/platform/`）
- **混音**：双流环形缓冲窗口对齐（参考 Meetily `pipeline.rs`：窗口零填充、溢出告警）；RMS 闪避 + 软削波（参考 `audio_processing.rs` 的 True-Peak 限幅 10ms lookahead、EBU R128 响度，Rust 侧用 `ebur128` crate）
- **流式 VAD**：sherpa-onnx VAD 端点检测，替代旧版固定 3 秒块
- **录音器**：增量 checkpoint 保存（30s 一段，崩溃可恢复，参考 Meetily `incremental_saver.rs`）；音频文件入 SQLite 索引
- **设备/权限**：设备枚举（输入/回环候选）、macOS 麦克风+屏幕录制权限引导、Windows WASAPI loopback

### 8.2 ASR 域（talksage-asr）

- **统一运行时**：sherpa-onnx（Rust 绑定），模型为 ONNX（从 k2-fsa 官方 HF 仓库下载，`models/` 清单 + 下载脚本）
- **双流式引擎**：
  - client（英文）→ streaming zipformer（en）或 streaming whisper 变体
  - user（中文）→ streaming paraformer-zh
- **流式接口**：`StreamingASREngine`：`accept(audio_chunk) -> Vec<PartialResult>`（增量出字）
- **后端探测**：启动时探测 CUDA（NVIDIA）/ CoreML（Apple GPU）/ CPU，自动选择推理后端与模型档位（CPU 用小模型，GPU 可升档）
- **说话人 embedding**：3dspeaker 类 embedding 模型；实时双流隐式分离为主（麦=我/回环=客户），回环内多人用 embedding 聚类；离线（导入/重转写）做完整 diarization
- **后处理**：增量文本清洗（重复/幻觉），参考 Meetily `clean_repetitive_text`；`no_speech` 阈值调参防幻觉

### 8.3 管道与插件（talksage-pipeline / talksage-plugins）

- **插件抽象**：`Plugin` trait：`should_trigger(segment) -> bool`、`analyze_stream(segment, ctx) -> AsyncStream<PluginResult>`（骨架→最终，`result_id` 原地更新，沿用旧版模式）
- **插件清单（v2 全量）**：
  - `term_explainer`：英文缩写 → 本地骨架 + LLM 终稿（冷却 + 会话去重）
  - `brief_retriever`：客户发言 → 知识库片段（冷却）
  - `translator`：中英互译，LLM 流式输出（低延迟 provider 如 Groq 优先）
  - `key_point_extractor`：关键短语/需求/技术方案要点（本地规则 + LLM 增强）
  - `notes`：会后纪要（模板化，见 §8.6）
- **上下文状态**：话题 / 未决问题 / 近期决策跟踪（沿用旧版启发式，可 LLM 增强）

### 8.4 会话域（talksage-session）

- SQLite schema（沿旧版扩展）：`sessions`（含录音文件路径、workspace、user）、`segments`（含 speaker_id、embedding 摘要）、`terms`、`translations`、`key_points`、`notes`
- Markdown 导出（增量追加，不再全量重写）
- 历史检索：SQL LIKE + 可选全文索引（预留）
- 迁移机制：schema_version + WAL checkpoint 清理（参考 Meetily `database/manager.rs` 的 WAL 恢复与旧库迁移）

### 8.5 LLM 域（talksage-llm）

- `LLMProvider` trait：`complete` / `stream`（OpenAI 兼容：DeepSeek/Kimi/Groq/MiniMax/Ollama…）
- 流式翻译/术语填充走 `stream`；后台任务带取消（CancellationToken，参考 Meetily `summary/service.rs`）

### 8.6 纪要模板化（talksage-plugins/notes）

- JSON 模板定义（沿用 Meetily `summary/templates/types.rs` 的 schema：name/description/sections[title/instruction/format/item_format]）
- 内置模板：标准会议 / 谈判记录 / 技术评审 / 每日站会
- 运行时校验 + 自动生成分节 LLM 指令 + 表格化 Action Items（owner/due/转写引用/时间戳）

### 8.7 知识库（talksage-knowledge）

- 本地 `.md/.txt` 文件夹，Jaccard 关键词检索（沿用旧版，零依赖）
- 预留向量化升级（本地 embedding 模型或 OpenAI 兼容 embeddings 接口）

---

## 9. 事件协议（与传输无关）

所有领域事件为纯数据（serde 序列化），IPC 与 WS 传同一结构：

```rust
// talksage-core/src/events.rs
enum DomainEvent {
    Segment { speaker_id: u32, text: String, is_partial: bool, ts_ms: u64 },
    Term { result_id: String, status: Skeleton|Final, content: String },
    Translation { result_id: String, status: Skeleton|Final, direction: ZhEn|EnZh, content: String },
    KeyPoint { result_id: String, status: Skeleton|Final, category: String, content: String },
    Brief { source: String, text: String },
    State { topic: String, open_questions: Vec<String>, decisions: Vec<String> },
    Status { stage: AsrLoading|AsrReady|Recording|Importing, message: String },
    Level { mic_rms: f32, loopback_rms: f32 },
    // …
}
```

前端订阅流：`subscribe(handler) -> Unsubscribe`（IPC 事件 / WS 消息适配器统一）。

---

## 10. 前端设计（web/）

- **分区**：上下文 / 实时转写（说话人着色、增量渲染） / 术语卡片 / 翻译区 / 要点区 / 简报区；虚拟化长转写（参考 Meetily `VirtualizedTranscriptView`）
- **统一 API 抽象**：`lib/api.ts` 定义 `AudioCapture` / `SessionApi` / `HistoryApi` / `SettingsApi` 接口，IPC 与 HTTP 两套实现
- **音频**：Tauri 模式音频全在壳内（前端只收事件 + 控制命令）；headless 模式麦克风走 `getUserMedia` + WS 上行
- **状态管理**：轻量 context + reducer（参考 Meetily `SidebarProvider` 模式，无需重型框架）
- **录音回放**：Web Audio 播放（音频文件经宿主提供）

---

## 11. 多设备 / 团队预留（先不做，留接口）

| 能力 | MVP（Tauri） | 预留接口 |
|---|---|---|
| 传输层 | IPC | `Transport` trait（IPC / HTTP+WS 双实现） |
| 鉴权 | 无（本地 IPC） | `AuthProvider` trait（headless 模式启用：token / 账号） |
| 会话归属 | 默认 workspace/user | `sessions.workspace_id` / `user_id` 字段 |
| 数据目录 | `~/.talksage/` | `~/.talksage/{workspace}/…` 结构 |
| 服务暴露 | 不启用 | `talksage serve --host 0.0.0.0 --token …`（默认 127.0.0.1） |

---

## 12. 平台层（Windows / macOS）

| 能力 | Windows | macOS |
|---|---|---|
| 回环采集 | WASAPI loopback（壳内 / capture-agent） | ScreenCaptureKit（macOS 13+）或 BlackHole 虚拟声卡 |
| 权限 | 麦克风；无需额外（回环走 WASAPI） | 麦克风 + 屏幕录制（SCK 必需）权限引导 |
| GPU | NVIDIA CUDA（ONNX Runtime CUDA） | Apple Metal / CoreML |
| 分发 | NSIS 安装包（WebView2 常驻） | dmg（WKWebView） |

---

## 13. 配置管理（talksage-config）

分层合并（简化版 DSH patch 思想）：**内置默认 → 用户 `talksage.toml` → 环境变量 → CLI 参数**。配置域：

```toml
[asr]
client_engine = "zipformer-en"        # 或 whisper
user_engine = "paraformer-zh"
backend = "auto"                      # auto | cpu | cuda | metal

[audio]
mic_device = null
loopback_device = null                # 视频会议回环
ducking = { enabled = true, threshold = 0.04, factor = 0.35 }
vad = { redemption_ms = 2000, pre_pad_ms = 300, post_pad_ms = 400, min_speech_ms = 250 }

[llm]
default = "deepseek"
providers = { deepseek = { base_url = "…", model = "…", api_key = "" } }

[plugins]
term_explainer = { enabled = true, cooldown_seconds = 10 }
translator = { enabled = true }
brief_retriever = { enabled = true }
notes = { template = "standard_meeting" }

[session]
sqlite = true
export_markdown = true
record_audio = true                    # 录音回放依赖

[privacy]
recording_consent_accepted = false

[server]                              # headless 模式（预留）
enabled = false
host = "127.0.0.1"
port = 8080
token = ""
```

---

## 14. 安全与隐私

- 音频、转写、会话**默认全本地**，不上传（同旧版承诺）
- 录音同意弹窗（`privacy.recording_consent_accepted` 持久化）
- headless 模式默认 127.0.0.1；对外开放需显式配置 + token（DSH 同款姿态：无 TLS/无鉴权的 v1 仅限回环）
- API Key 仅存本地配置

---

## 15. 里程碑

| 阶段 | 内容 | 交付 |
|---|---|---|
| **M0 骨架** | Rust workspace + launcher + Tauri 壳 + React 空壳 + IPC hello-world + 配置加载 | 可启动空应用 |
| **M1 实时转写** | 采集（麦+回环）→ 流式 VAD → streaming 双引擎 → 增量上屏（说话人双流着色） | 核心价值闭环 |
| **M2 会议辅助** | 术语/简报/要点/上下文 + 翻译插件 + SQLite 会话 + 历史页 | 旧版功能等价 + 翻译 |
| **M3 产品化** | 录音回放、重转写、纪要模板化、设置页/向导、双平台打包 | 可日常使用 |
| **M4（预留）** | headless 服务 + capture-agent + 鉴权 → 多设备/团队 | 浏览器/手机访问 |

---

## 16. 风险与开放问题

| 风险 | 缓解 |
|---|---|
| streaming 模型中文准确率 vs 离线 paraformer | 双引擎可切换；导入/重转写用 SenseVoice/离线模型补高质量 |
| macOS ScreenCaptureKit 权限与稳定性 | 提供 BlackHole 虚拟声卡降级路径（Meetily 同款） |
| 说话人分离在实时场景精度有限 | 分层策略：双流隐式为主，embedding 聚类增强，离线完整 diarization |
| Tauri 2 + WebView2/WKWebView 兼容 | 前端避免实验性 API；提供 headless 浏览器访问作为降级载体 |
| LLM 延迟拖慢实时体验 | 快慢路径分离；翻译/术语用低延迟 provider（Groq/Ollama） |
| 模型体积与分发 | models/ 清单 + 首次运行下载（同旧版模式）；GPU 升档可选 |

---

## 17. 参考项目与借鉴点

| 项目 | 借鉴点 |
|---|---|
| [DeepSeek Harness（DSH）](https://github.com/deepseek-ai/DeepSeek-Harness) | CLI launcher + profile；能力按域拆包；宿主默认回环安全；/api + WS 事件；配置分层；同一 UI 跑浏览器或原生壳走 IPC |
| [Meetily](https://github.com/Zackriya-Solutions/meetily) | Tauri 2 同场景验证；流式 VAD 调参；环形缓冲混音；True-Peak 限幅/EBU R128；增量 checkpoint 保存；纪要模板系统；WAL 恢复；whisper 调参 |
| [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) | 统一 ASR 运行时：streaming paraformer/zipformer、SenseVoice、说话人 embedding；Rust 绑定；CUDA/CoreML 多后端 |
