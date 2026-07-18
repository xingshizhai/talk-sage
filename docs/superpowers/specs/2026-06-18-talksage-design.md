# TalkSage — 实时 AI 会议秘书 设计文档

**日期：** 2026-06-18（初版） / **修订：** 2026-07-18  
**状态：** 实施中（Phase 1 MVP + P0/P1/P2 产品化增强已落地）

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
AudioHub（麦 + 回环）
    → DualASREngine（local: Whisper+FunASR / cloud: Whisper API）
    → CrosstalkFilter（串音抑制）
    → PluginBus（StateTracker + 并行插件，支持 skeleton→final）
    → SessionStore / NotesGenerator
    → PySide6 侧边栏 UI
```

### 各层职责

| 模块 | 职责 |
|------|------|
| `AudioHub` | 16 kHz 双路采集：麦克风=`user`，系统回环=`client`；默认约 3 秒分块 |
| `ASREngine` / `factory` | `mode: local \| cloud`；本地双引擎或 OpenAI 兼容云端转写 |
| `CrosstalkFilter` | 用户转写与近期客户转写高度相似时丢弃（抑制回环漏进麦克风） |
| `PluginBus` | 维护 `ConversationContext` + `StateTracker`；`analyze_stream` 渐进推送结果 |
| `Plugins` | `term_explainer`、`brief_retriever`；（规划）翻译 / 技术评估 / 谈判分析 |
| `KnowledgeBase` | 本地 `.md/.txt` 关键词检索（无强制 embedding API） |
| `SessionStore` | 会话自动落盘 `~/.talksage/sessions/*.md` |
| `NotesGenerator` | 会后基于转写+上下文+术语生成纪要 |
| `UI` | 侧边栏：上下文 / 转写 / 术语 / 简报；向导、同意弹窗、屏享排除 |

---

## 三、ASR 设计

### 模式

| `transcribe.mode` | 客户（英文） | 用户（中文） |
|-------------------|-------------|-------------|
| `local`（默认） | `FasterWhisperEngine` | `FunASREngine`（Paraformer） |
| `cloud` | `OpenAICloudEngine`（language=en） | `OpenAICloudEngine`（language=zh） |

云端模式当前为**分块批处理**（上传 WAV 到 `/audio/transcriptions`），非 WebSocket 真流式。真流式列为后续优化。

### 接口

```python
class ASREngine(ABC):
    def warmup(self) -> None: ...
    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None: ...
```

启动时后台 `warmup()`，状态栏显示：`ASR 加载中…` → `ASR 就绪`。  
推理在 `run_in_executor` 中执行，避免阻塞 asyncio。

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
┌─────────────────────┐
│ TalkSage    [状态]   │  ASR 就绪 / 监听中；屏享可排除（Windows）
├─────────────────────┤
│ [开始监听] [生成纪要] │
├─────────────────────┤
│ 🧭 上下文            │  话题 / 未决问题 / 决策
├─────────────────────┤
│ 🎙 实时转写          │  我 / 客户
├─────────────────────┤
│ 📖 术语             │  骨架 → 终稿原地更新
├─────────────────────┤
│ 💡 简报             │  知识库命中片段
└─────────────────────┘
```

**已实现交互：**

- 首次启动：Setup Wizard（ASR / LLM / 知识库）  
- 首次监听：录音同意确认  
- Windows：`hide_from_screen_share` → `SetWindowDisplayAffinity`  
- 停止监听后可「生成纪要」

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

**串音抑制：** 转写后 Jaccard 相似度超过阈值且在时间窗内 → 丢弃用户侧片段（可配置 `audio.crosstalk.*`）。

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
4. **会话落盘**：仅本机 `~/.talksage/sessions/`  

---

## 九、配置要点（`~/.talksage/config.yaml`）

```yaml
transcribe:
  mode: local                 # local | cloud
  cloud:
    api_key: ""
    base_url: https://api.openai.com/v1
    model: whisper-1
  client: { model: small, device: cpu, compute_type: int8, vad_filter: true }
  user: { model: paraformer-zh, device: cpu }
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
  notes_llm: null             # null = llm.default

privacy:
  recording_consent_accepted: false
  hide_from_screen_share: true

setup:
  completed: false

audio:
  crosstalk:
    similarity_threshold: 0.6
    window_seconds: 8
```

---

## 十、项目结构（当前）

```
talk-sage/
├── main.py
├── requirements.txt
├── README.md
├── core/
│   ├── models.py                 # Segment / PluginResult / Context / State
│   ├── audio_hub.py
│   ├── pipeline.py
│   ├── plugin_bus.py
│   ├── echo_filter.py
│   ├── session_store.py
│   ├── conversation_state.py     # StateTracker
│   ├── knowledge_base.py
│   ├── notes_generator.py
│   ├── transcribe_engine.py      # 兼容别名 → FasterWhisperEngine
│   └── asr/
│       ├── base.py
│       ├── factory.py
│       ├── dual_engine.py
│       ├── faster_whisper_engine.py
│       ├── funasr_engine.py
│       └── openai_cloud_engine.py
├── plugins/
│   ├── base.py
│   ├── term_explainer.py
│   └── brief_retriever.py
├── llm/
│   ├── base.py
│   └── openai_compat.py
├── ui/
│   ├── main_window.py
│   ├── setup_wizard.py
│   ├── consent_dialog.py
│   ├── screen_share.py
│   ├── style.py
│   └── sections/
│       ├── context.py
│       ├── transcript.py
│       ├── terms.py
│       └── suggestions.py
├── config/
│   ├── manager.py
│   ├── defaults.yaml
│   └── config.template.yaml
├── tests/                        # 100+ 单元测试
└── docs/superpowers/
    ├── specs/                    # 本设计文档
    └── plans/                    # 实施计划与进度
```

---

## 十一、开发路线图与完成度

### Phase 1 — MVP（已完成）

- [x] 双路音频（麦克风 + loopback）  
- [x] 本地双引擎 ASR（英文 Whisper + 中文 FunASR）  
- [x] PluginBus + 术语解释插件  
- [x] 侧边栏转写 + 术语区  
- [x] 配置管理 + OpenAI 兼容多 LLM  

### P0 — 体验与合规（已完成，2026-07）

- [x] 术语去重 + 冷却  
- [x] 启动 warmup + ASR 状态提示  
- [x] 录音同意弹窗 + README 免责  
- [x] Windows 屏幕共享排除  

### P1 — 架构增强（已完成，2026-07）

- [x] ASR `local | cloud` 工厂  
- [x] 插件骨架 → 终稿（`analyze_stream`）  
- [x] 会话自动 Markdown 落盘  
- [x] 双路串音抑制 + ASR 线程池  

### P2 — 产品化（已完成，2026-07）

- [x] Setup Wizard  
- [x] ConversationState（启发式）  
- [x] 可选本地知识库 + brief_retriever  
- [x] 会后生成纪要  

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

产品能力借鉴 [OpenOats](https://github.com/yazinsai/OpenOats) 的思路（多 ASR 后端、门控节流、屏享隐形、同意声明、会话落盘），但产品主线保持「中英会议辅助 + 术语/简报/谈判」，而非通用笔记 RAG 会议助手。
