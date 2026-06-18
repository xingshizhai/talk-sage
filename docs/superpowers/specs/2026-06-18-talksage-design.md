# TalkSage — 实时 AI 会议秘书 设计文档

**日期：** 2026-06-18
**状态：** 已批准

---

## 一、产品定位

TalkSage 是一款跨平台桌面应用，面向需要与英文客户沟通的中文商务/技术用户。它在会议、谈判、面对面沟通时后台运行，实时采集双路音频，自动识别专业术语、翻译对话、评估技术方案与谈判条款，通过侧边栏静默提示用户——相当于一个实时 AI 秘书。

**目标用户：** 个人用户（中文母语，需与英文客户沟通的商务/技术人员）
**支持平台：** Windows、macOS、Ubuntu

---

## 二、整体架构

插件化架构，核心链路：

```
音频采集(AudioHub) → 转写引擎(TranscribeEngine) → 插件总线(PluginBus) → PySide6 侧边栏 UI
```

### 各层职责

| 模块 | 职责 |
|------|------|
| `AudioHub` | 同时采集麦克风（用户）和系统音频 loopback（客户），打角色标签后送入队列 |
| `TranscribeEngine` | 将音频片段转为带角色标注的文本，可配置本地 Whisper 或云端 API |
| `PluginBus` | 接收转写片段，广播给所有已启用插件，插件并行异步执行 |
| `Plugins` | 各自独立分析，调用 LLM，返回结构化结果 |
| `UI` | PySide6 侧边栏，分四区显示结果，通过 Qt 信号接收插件输出 |

---

## 三、插件系统

### 抽象接口

```python
class AnalyzerPlugin:
    name: str
    display_name: str
    ui_section: str  # transcript / terms / translation / suggestions

    def should_trigger(self, segment: TranscriptSegment) -> bool: ...
    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult: ...
```

### 内置插件

| 插件 | 触发条件 | 输出 | UI 分区 |
|------|---------|------|---------|
| `term_explainer` | 检测到英文专业词汇（NPI、BOQ、MOQ 等） | 中文解释 + 行业背景 | 术语区 |
| `translator` | 客户说英文 / 用户说中文 | 对应语言翻译 | 翻译区 |
| `tech_evaluator` | 客户提出技术方案或参数 | 可行性分析、潜在风险 | 建议区 |
| `negotiation_analyzer` | 客户提出条款、价格、交期 | 条款评估、谈判策略建议 | 建议区 |

`ConversationContext` 保存近 N 分钟对话历史，供插件理解上下文。

---

## 四、UI 侧边栏

侧边栏独立窗口，与会议软件或面对面对话并排显示，分四个固定分区：

```
┌─────────────────────┐
│  🎙 实时转写          │  滚动显示最近对话，带角色标注（用户/客户）
├─────────────────────┤
│  📖 术语             │  最新识别到的专业词汇及中文解释
├─────────────────────┤
│  🌐 翻译             │  上一句话的对应翻译
├─────────────────────┤
│  💡 建议             │  技术评估 + 谈判分析，按重要性排序，可点击展开
└─────────────────────┘
```

**交互细节：**
- 每个分区可折叠/展开
- 建议区条目可点击查看详细分析
- 侧边栏宽度可拖拽调整
- 系统托盘图标控制录音开始/暂停

---

## 五、音频采集

### 双路采集

| 音频流 | 角色标注 | 采集方式 |
|--------|---------|---------|
| 麦克风 | `speaker: user` | `sounddevice` 标准输入 |
| 系统音频 | `speaker: client` | 平台 loopback（见下表） |

### 跨平台 Loopback 方案

| 系统 | 方案 |
|------|------|
| Windows | WASAPI loopback，`sounddevice` 原生支持 |
| macOS | BlackHole 虚拟声卡（首次使用引导安装） |
| Ubuntu | PulseAudio monitor source，自动检测 |

### 转写输出格式

```python
TranscriptSegment(
    speaker="client",       # user / client
    text="our NPI schedule starts in Q3",
    language="en",
    timestamp=1234567890,
)
```

每 2-3 秒切一个片段，VAD 检测过滤静音段。

---

## 六、LLM 集成

### 提供商抽象

```python
class LLMProvider:
    async def complete(self, prompt: str, system: str) -> str: ...
    async def stream(self, prompt: str, system: str) -> AsyncIterator[str]: ...
```

**两种实现：**
- `AnthropicProvider`：Claude API 专用
- `OpenAICompatProvider`：通用 OpenAI 格式，覆盖所有兼容提供商

### 支持的提供商

| 提供商 | base_url | 推荐用途 |
|--------|---------|---------|
| Claude（Anthropic） | `api.anthropic.com` | 谈判分析、技术评估 |
| OpenAI GPT | `api.openai.com` | 通用备选 |
| Groq | `api.groq.com/openai/v1` | 低延迟翻译、术语解释 |
| DeepSeek | `api.deepseek.com/v1` | 技术评估、中文理解 |
| Kimi（月之暗面） | `api.moonshot.cn/v1` | 长上下文 |
| MiniMax | `api.minimax.chat/v1` | 通用中文 |
| 阿里通义 | `dashscope.aliyuncs.com/...` | 中文备选 |
| Ollama（本地） | `localhost:11434` | 离线/隐私场景 |

### 配置文件 `~/.talksage/config.yaml`

```yaml
transcribe:
  mode: local            # local / api
  model: base

llm:
  default: claude
  providers:
    claude:
      api_key: sk-...
      model: claude-sonnet-4-6
    deepseek:
      base_url: https://api.deepseek.com/v1
      api_key: sk-...
      model: deepseek-chat
    kimi:
      base_url: https://api.moonshot.cn/v1
      api_key: sk-...
      model: moonshot-v1-32k
    groq:
      base_url: https://api.groq.com/openai/v1
      api_key: gsk-...
      model: llama3-70b-8192
    ollama:
      base_url: http://localhost:11434
      model: llama3

plugins:
  term_explainer:
    enabled: true
    llm: groq
  translator:
    enabled: true
    llm: groq
  tech_evaluator:
    enabled: true
    llm: claude
  negotiation_analyzer:
    enabled: true
    llm: claude
```

---

## 七、项目结构

```
talk-sage/
├── main.py
├── core/
│   ├── audio_hub.py
│   ├── transcribe_engine.py
│   ├── plugin_bus.py
│   └── conversation_context.py
├── plugins/
│   ├── base.py
│   ├── term_explainer.py
│   ├── translator.py
│   ├── tech_evaluator.py
│   └── negotiation_analyzer.py
├── llm/
│   ├── base.py
│   ├── anthropic_provider.py
│   └── openai_compat.py
├── ui/
│   ├── main_window.py
│   ├── sections/
│   │   ├── transcript.py
│   │   ├── terms.py
│   │   ├── translation.py
│   │   └── suggestions.py
│   └── setup_wizard.py
├── config/
│   ├── manager.py
│   └── defaults.yaml
└── tests/
    ├── test_plugin_bus.py
    ├── test_transcribe_engine.py
    └── fixtures/
```

---

## 八、开发路线图

### Phase 1 — MVP（核心管道）
1. 单路麦克风采集
2. 本地 Whisper 转写
3. PluginBus 基础框架
4. 术语解释插件
5. 基础侧边栏（仅术语区）

### Phase 2 — 功能完整
1. 双路音频（系统 loopback）
2. 翻译插件 + 翻译区
3. 技术评估 + 谈判分析插件
4. 完整四分区 UI
5. 配置文件 + 设置向导

### Phase 3 — 打磨与扩展
1. 云端转写 API
2. 所有 LLM 提供商接入
3. 三平台打包（PyInstaller）
4. 会议记录导出（Markdown/PDF）
