# TalkSage — 实时 AI 会议秘书 设计文档

**日期：** 2026-06-18（初版） / **修订：** 2026-07-18（含 Meetily 启发优化）  
**状态：** 实施中（Phase 1 + P0–P2 + 音频/ASR/会话库增强已落地）

---

## 一、产品定位

TalkSage 是一款跨平台桌面应用，面向需要与英文客户沟通的中文商务/技术用户。它在会议、谈判、面对面沟通时后台运行，实时采集双路音频，自动识别专业术语、检索客户简报、跟踪对话上下文，并在会后生成纪要——相当于一个实时 AI 秘书。

**目标用户：** 个人用户（中文母语，需与英文客户沟通的商务/技术人员）  
**支持平台：** Windows、macOS、Ubuntu  
**差异化（相对 OpenOats 等通用会议助手）：** 中英双引擎 ASR、术语解释、客户 brief 检索、谈判/技术场景插件化扩展。

---

## 二、整体架构

插件化管道，核心链路：

```
AudioHub（麦 + 回环；ducking / soft-limit）
    → DualASREngine（local: Whisper|Parakeet + FunASR / cloud: Whisper API）
    → CrosstalkFilter（文本串音抑制）
    → PluginBus（StateTracker + 并行插件，skeleton→final）
    → SessionStore（Markdown）+ SessionDatabase（SQLite）
    → NotesGenerator / OfflineTranscriber（导入）
    → PySide6 侧边栏 UI
```

### 各层职责

| 模块 | 职责 |
|------|------|
| `AudioHub` | 16 kHz 双路采集；可选麦/回环设备；麦克风闪避 + 防削波 |
| `device_probe` | GPU 探测（`device: auto`）、枚举输入设备 |
| `ASREngine` / `factory` | `mode: local \| cloud`；英文引擎可选 faster-whisper / Parakeet |
| `CrosstalkFilter` | 用户转写与近期客户转写高度相似时丢弃 |
| `PluginBus` | `ConversationContext` + `StateTracker`；`analyze_stream` |
| `Plugins` | `term_explainer`、`brief_retriever`；（规划）翻译 / 评估 / 谈判 |
| `KnowledgeBase` | 本地 `.md/.txt` 关键词检索 |
| `SessionStore` | Markdown 落盘 `~/.talksage/sessions/*.md` |
| `SessionDatabase` | SQLite `sessions.db`：可检索历史 |
| `import_audio` | 导入录音离线分块转写 |
| `NotesGenerator` | 会后纪要 |
| `UI` | 上下文 / 转写 / 术语 / 简报；向导、设置、历史、导入 |

---

## 三、ASR 设计

### 模式

| `transcribe.mode` | 客户（英文） | 用户（中文） |
|-------------------|-------------|-------------|
| `local`（默认） | `faster-whisper`（默认）或 `parakeet` | `FunASREngine`（Paraformer） |
| `cloud` | `OpenAICloudEngine`（en） | `OpenAICloudEngine`（zh） |

| `transcribe.client.engine` | 说明 | 依赖 |
|----------------------------|------|------|
| `faster-whisper` | 默认；`model` 为 tiny/base/small/… | `faster-whisper` |
| `parakeet` | NVIDIA Parakeet ONNX，通常更快 | 可选：`pip install "onnx-asr[cpu,hub]"` |

`device` / `compute_type` 支持 `auto`：启动时由 `device_probe` 探测 CUDA，GPU 推荐 `float16`，否则 CPU `int8`。

云端模式为**分块批处理**（`/audio/transcriptions`），非 WebSocket 真流式。

### 接口

```python
class ASREngine(ABC):
    def warmup(self) -> None: ...
    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None: ...
```

启动后台 `warmup()`；状态栏：`ASR 加载中…` → `ASR 就绪`。  
推理在 `run_in_executor` 中执行。导入音频走 `OfflineTranscriber`（同一 `ASREngine`）。

---

## 四、插件系统

### 抽象接口

```python
class AnalyzerPlugin(ABC):
    name: ClassVar[str]
    display_name: ClassVar[str]
    ui_section: ClassVar[str]  # transcript / terms / translation / suggestions

    def should_trigger(self, segment: TranscriptSegment) -> bool: ...
    async def analyze(...) -> PluginResult: ...
    async def analyze_stream(...) -> AsyncGenerator[PluginResult, None]:
        # 默认：yield analyze()；可先 skeleton 后 final
```

### `PluginResult`

| 字段 | 说明 |
|------|------|
| `content` | 展示文本 |
| `result_id` | 稳定 ID，供 UI 原地更新 |
| `status` | `skeleton` \| `final` |
| `ui_section` | 路由到对应 UI 分区 |
| `priority` | 展示优先级 |

### 已实现插件

| 插件 | 触发条件 | 输出 | UI |
|------|---------|------|-----|
| `term_explainer` | 客户英文句含未见缩写；冷却 + 会话去重 | 先 `NPI = …`，再 LLM 终稿 | 术语区 |
| `brief_retriever` | 客户发言命中知识库（冷却） | 相关 brief 片段 | 简报区 |

### 规划中插件

| 插件 | 触发条件 | 输出 | UI |
|------|---------|------|-----|
| `translator` | 客户英文 / 用户中文 | 对向翻译 | 翻译区（待建） |
| `tech_evaluator` | 技术方案/参数 | 可行性与风险 | 建议区 |
| `negotiation_analyzer` | 条款、价格、交期 | 策略建议 | 建议区 |

### 对话状态

`StateTracker`（启发式，无需 LLM）维护 `ConversationState`：

- `topic` — 近期客户发言主题提示  
- `open_questions` — 问句  
- `recent_decisions` — 含 agreed / proceed with 等决策表述  

通过 `ConversationContext.state` 供插件与纪要使用；UI「上下文」区实时展示。

---

## 五、UI 侧边栏

```
┌──────────────────────────────────────┐
│ TalkSage                     [状态]   │  ASR 就绪 / 监听中
├──────────────────────────────────────┤
│ [监听] [纪要] [导入] [历史] [设置]     │
├──────────────────────────────────────┤
│ 🧭 上下文     话题 / 未决问题 / 决策    │
├──────────────────────────────────────┤
│ 🎙 实时转写   我 / 客户                 │
├──────────────────────────────────────┤
│ 📖 术语      骨架 → 终稿原地更新         │
├──────────────────────────────────────┤
│ 💡 简报      知识库命中片段             │
└──────────────────────────────────────┘
```

**已实现交互：**

- 首次启动：Setup Wizard（ASR / LLM / 知识库）  
- 首次监听：录音同意确认  
- Windows：`hide_from_screen_share` → `SetWindowDisplayAffinity`  
- 「设置」：麦克风 / 回环 / 英文引擎 / GPU 设备 / 闪避与防削波  
- 「导入音频」：离线转写并写入 sessions  
- 「历史」：SQLite 会话浏览与关键词搜索  
- 停止监听后「生成纪要」

**尚未实现：** 分区折叠、宽度拖拽、系统托盘、独立翻译区。

---

## 六、音频采集与串音

| 音频流 | 角色 | 采集 |
|--------|------|------|
| 麦克风 | `user` | `sounddevice` InputStream |
| 系统音频 | `client` | Loopback / Stereo Mix / monitor（可自动检测） |

| 系统 | Loopback |
|------|----------|
| Windows | WASAPI / Stereo Mix |
| macOS | BlackHole（需用户安装） |
| Ubuntu | PulseAudio monitor |

**音频处理（Meetily 启发）：**

| 能力 | 配置 | 说明 |
|------|------|------|
| 麦克风闪避 | `audio.ducking.*` | 回环响度高时压低麦增益 |
| 防削波 | `audio.soft_limit.*` | 峰值软限制，减轻失真 |
| 文本串音 | `audio.crosstalk.*` | 转写后 Jaccard 去重 |

---

## 七、LLM 与知识库

### LLM

- 接口：`LLMProvider.complete` / 可选 `stream`  
- 实现：`OpenAICompatProvider`（DeepSeek / Kimi / Groq / Ollama 等）  
- Claude 专用 Provider 仍为规划项（配置中已预留）

| 提供商 | 推荐用途 |
|--------|---------|
| DeepSeek | 术语解释、纪要（中文） |
| Groq | 低延迟术语解释 |
| Kimi | 长上下文 |
| Ollama | 全本地 |
| Claude | 评估 / 谈判（规划） |

### 知识库

- 配置：`knowledge_base.enabled` + `folder`  
- 索引：按 Markdown 标题/段落切块  
- 检索：本地关键词 Jaccard（无 Voyage/embedding 依赖）  
- 命中后由 `brief_retriever` 推到简报区  

后续可升级为 embedding + rerank，接口保持 `KnowledgeBase.search`。

---

## 八、隐私与合规

1. **录音同意**：首次监听前弹窗；`privacy.recording_consent_accepted`  
2. **数据出网**：默认 ASR 本地，音频不上云；启用插件时仅发送**文本**到 LLM  
3. **屏享隐形**：Windows 排除捕获；macOS/Linux 建议「共享特定窗口」  
4. **会话落盘**：本机 `sessions/*.md` + 可选 `sessions.db`  

---

## 九、配置要点（`~/.talksage/config.yaml`）

```yaml
transcribe:
  mode: local                 # local | cloud
  cloud:
    api_key: ""
    base_url: https://api.openai.com/v1
    model: whisper-1
  client:
    engine: faster-whisper    # faster-whisper | parakeet
    model: small              # 或 nemo-parakeet-tdt-0.6b-v3
    device: auto
    compute_type: auto
    vad_filter: true
  user: { model: paraformer-zh, device: auto }
  loopback: { enabled: false, device: null }

llm:
  default: deepseek
  providers: { ... }

plugins:
  term_explainer:
    enabled: true
    llm: deepseek
    cooldown_seconds: 10

knowledge_base:
  enabled: false
  folder: ""

session:
  auto_save: true
  sqlite: true
  notes_llm: null

privacy:
  recording_consent_accepted: false
  hide_from_screen_share: true

setup:
  completed: false

audio:
  mic_device: null
  ducking: { enabled: true, threshold: 0.04, factor: 0.35 }
  soft_limit: { enabled: true, threshold: 0.95 }
  crosstalk: { similarity_threshold: 0.6, window_seconds: 8 }
```

---

## 十、项目结构（当前）

```
talk-sage/
├── main.py
├── requirements.txt
├── README.md
├── core/
│   ├── models.py
│   ├── audio_hub.py / audio_process.py / device_probe.py
│   ├── pipeline.py / plugin_bus.py / echo_filter.py
│   ├── session_store.py / session_db.py
│   ├── conversation_state.py / knowledge_base.py
│   ├── notes_generator.py / import_audio.py
│   └── asr/
│       ├── base.py / factory.py / dual_engine.py
│       ├── faster_whisper_engine.py / funasr_engine.py
│       ├── openai_cloud_engine.py / parakeet_engine.py
├── plugins/   # term_explainer, brief_retriever
├── llm/       # openai_compat
├── ui/
│   ├── main_window.py / setup_wizard.py / settings_dialog.py
│   ├── history_dialog.py / consent_dialog.py / screen_share.py
│   └── sections/  # context, transcript, terms, suggestions
├── config/
├── tests/                        # 120+ 单元测试
└── docs/superpowers/
```

---

## 十一、开发路线图与完成度

### Phase 1 — MVP（已完成）

- [x] 双路音频（麦克风 + loopback）  
- [x] 本地双引擎 ASR（英文 Whisper + 中文 FunASR）  
- [x] PluginBus + 术语解释插件  
- [x] 侧边栏转写 + 术语区  
- [x] 配置管理 + OpenAI 兼容多 LLM  

### P0 — 体验与合规（已完成）

- [x] 术语去重 + 冷却  
- [x] 启动 warmup + ASR 状态提示  
- [x] 录音同意弹窗 + README 免责  
- [x] Windows 屏幕共享排除  

### P1 — 架构增强（已完成）

- [x] ASR `local | cloud` 工厂  
- [x] 插件骨架 → 终稿（`analyze_stream`）  
- [x] 会话自动 Markdown 落盘  
- [x] 双路串音抑制 + ASR 线程池  

### P2 — 产品化（已完成）

- [x] Setup Wizard  
- [x] ConversationState（启发式）  
- [x] 可选本地知识库 + brief_retriever  
- [x] 会后生成纪要  

### Meetily 启发优化（已完成，2026-07）

- [x] GPU / `device: auto`  
- [x] 设置页：设备与 ASR 引擎  
- [x] 麦克风闪避 + 防削波  
- [x] 导入音频离线转写  
- [x] SQLite 会话库 + 历史 UI  
- [x] 可选 Parakeet 英文引擎  

### Phase 3 — 下一阶段（待做）

1. 翻译插件 + 翻译区 UI  
2. 技术评估 / 谈判分析插件（快慢模型分离）  
3. 云端 **流式** ASR（WebSocket）  
4. 知识库 embedding / rerank（可选）  
5. VAD 端点切分（替代固定 3 秒块）  
6. Claude 直连 Provider  
7. 三平台打包（PyInstaller）  
8. 系统托盘、分区折叠、宽度拖拽  

---

## 十二、参考与借鉴

| 项目 | 借鉴点 | TalkSage 差异 |
|------|--------|---------------|
| [OpenOats](https://github.com/yazinsai/OpenOats) | 门控节流、屏享隐形、同意声明、会话落盘、会中提示节奏 | 非通用笔记 RAG；主线为中英术语/简报/谈判 |
| [Meetily](https://github.com/Zackriya-Solutions/meetily) | 本地 ASR 性能（Parakeet）、混音/闪避、SQLite、导入重转写、安装体验 | 不做成通用纪要工具；侧边栏会中辅助优先 |

产品主线始终是：**中英会议辅助 + 术语解释 + 客户简报 + 可扩展谈判/技术插件**。
