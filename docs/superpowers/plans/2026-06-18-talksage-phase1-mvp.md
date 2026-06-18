# TalkSage Phase 1 MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a working MVP that captures microphone audio, transcribes with local Whisper, detects English technical terms, explains them in Chinese via LLM, and displays results in a PySide6 sidebar.

**Architecture:** Plugin-based pipeline — AudioHub feeds TranscribeEngine, which feeds PluginBus, which broadcasts to analyzer plugins. Results flow via Qt signals to a sidebar UI with four fixed sections. Each layer is independently testable via abstract interfaces.

**Tech Stack:** Python 3.11+, PySide6, faster-whisper, sounddevice, anthropic SDK, pyyaml, pytest, pytest-asyncio

---

## File Map

```
talk-sage/
├── main.py                          # App entry point
├── requirements.txt                 # Dependencies
├── core/
│   ├── __init__.py
│   ├── models.py                    # TranscriptSegment, PluginResult, ConversationContext
│   ├── audio_hub.py                 # Mic capture, VAD, segment queue
│   ├── transcribe_engine.py         # Whisper wrapper (local)
│   └── plugin_bus.py                # Plugin registry + broadcast
├── plugins/
│   ├── __init__.py
│   ├── base.py                      # AnalyzerPlugin abstract base
│   └── term_explainer.py            # Term detection + Chinese explanation
├── llm/
│   ├── __init__.py
│   ├── base.py                      # LLMProvider abstract base
│   └── openai_compat.py             # OpenAI-compatible provider (covers DeepSeek, Kimi, Groq, etc.)
├── config/
│   ├── __init__.py
│   ├── manager.py                   # Load/save ~/.talksage/config.yaml
│   └── defaults.yaml                # Default config values
├── ui/
│   ├── __init__.py
│   ├── main_window.py               # Sidebar QMainWindow
│   └── sections/
│       ├── __init__.py
│       ├── transcript.py            # Real-time transcript section widget
│       └── terms.py                 # Terms section widget
└── tests/
    ├── conftest.py                  # Shared fixtures
    ├── test_models.py
    ├── test_plugin_bus.py
    ├── test_term_explainer.py
    ├── test_llm_openai_compat.py
    ├── test_config_manager.py
    └── fixtures/
        └── sample_segment.py        # Test fixture factory
```

---

## Task 1: Project Setup

**Files:**
- Create: `requirements.txt`
- Create: `main.py`
- Create: `core/__init__.py`, `plugins/__init__.py`, `llm/__init__.py`, `config/__init__.py`, `ui/__init__.py`, `ui/sections/__init__.py`
- Create: `tests/conftest.py`

- [ ] **Step 1: Create virtual environment and install dependencies**

```bash
python -m venv .venv
# Windows:
.venv\Scripts\activate
# macOS/Linux:
source .venv/bin/activate
```

- [ ] **Step 2: Create `requirements.txt`**

```
PySide6>=6.7.0
faster-whisper>=1.0.0
sounddevice>=0.4.6
anthropic>=0.28.0
openai>=1.30.0
pyyaml>=6.0.1
pytest>=8.0.0
pytest-asyncio>=0.23.0
pytest-qt>=4.4.0
numpy>=1.26.0
```

- [ ] **Step 3: Install dependencies**

```bash
pip install -r requirements.txt
```

Expected: all packages install without error.

- [ ] **Step 4: Create all `__init__.py` files**

```bash
# Run from project root
mkdir -p core plugins llm config ui/sections tests/fixtures
touch core/__init__.py plugins/__init__.py llm/__init__.py config/__init__.py
touch ui/__init__.py ui/sections/__init__.py
touch tests/__init__.py tests/fixtures/__init__.py
```

- [ ] **Step 5: Create minimal `main.py`**

```python
import sys
from PySide6.QtWidgets import QApplication
from ui.main_window import MainWindow


def main():
    app = QApplication(sys.argv)
    window = MainWindow()
    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
```

- [ ] **Step 6: Create `tests/conftest.py`**

```python
import pytest


@pytest.fixture
def anyio_backend():
    return "asyncio"
```

- [ ] **Step 7: Commit**

```bash
git add .
git commit -m "feat: project scaffold and dependencies"
```

---

## Task 2: Core Data Models

**Files:**
- Create: `core/models.py`
- Create: `tests/test_models.py`
- Create: `tests/fixtures/sample_segment.py`

- [ ] **Step 1: Write failing tests**

Create `tests/test_models.py`:

```python
import time
from core.models import TranscriptSegment, PluginResult, ConversationContext


def test_transcript_segment_creation():
    seg = TranscriptSegment(
        speaker="client",
        text="our NPI schedule starts in Q3",
        language="en",
        timestamp=1234567890.0,
    )
    assert seg.speaker == "client"
    assert seg.text == "our NPI schedule starts in Q3"
    assert seg.language == "en"
    assert seg.timestamp == 1234567890.0


def test_transcript_segment_default_timestamp():
    before = time.time()
    seg = TranscriptSegment(speaker="user", text="hello", language="zh")
    after = time.time()
    assert before <= seg.timestamp <= after


def test_plugin_result_creation():
    result = PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = 新产品导入流程 (New Product Introduction)",
        priority=1,
    )
    assert result.plugin_name == "term_explainer"
    assert result.ui_section == "terms"
    assert "NPI" in result.content
    assert result.priority == 1


def test_conversation_context_add_and_recent():
    ctx = ConversationContext(max_segments=3)
    for i in range(5):
        ctx.add(TranscriptSegment(speaker="client", text=f"sentence {i}", language="en"))
    recent = ctx.recent()
    assert len(recent) == 3
    assert recent[-1].text == "sentence 4"


def test_conversation_context_as_text():
    ctx = ConversationContext(max_segments=10)
    ctx.add(TranscriptSegment(speaker="client", text="Hello", language="en"))
    ctx.add(TranscriptSegment(speaker="user", text="你好", language="zh"))
    text = ctx.as_text()
    assert "[client]" in text
    assert "Hello" in text
    assert "[user]" in text
    assert "你好" in text
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_models.py -v
```

Expected: `ImportError: No module named 'core.models'`

- [ ] **Step 3: Implement `core/models.py`**

```python
import time
from dataclasses import dataclass, field
from collections import deque
from typing import Literal


@dataclass
class TranscriptSegment:
    speaker: Literal["user", "client"]
    text: str
    language: str
    timestamp: float = field(default_factory=time.time)


@dataclass
class PluginResult:
    plugin_name: str
    ui_section: Literal["transcript", "terms", "translation", "suggestions"]
    content: str
    priority: int = 0  # higher = more prominent in UI


class ConversationContext:
    def __init__(self, max_segments: int = 50):
        self._segments: deque[TranscriptSegment] = deque(maxlen=max_segments)

    def add(self, segment: TranscriptSegment) -> None:
        self._segments.append(segment)

    def recent(self, n: int | None = None) -> list[TranscriptSegment]:
        segments = list(self._segments)
        return segments if n is None else segments[-n:]

    def as_text(self) -> str:
        return "\n".join(
            f"[{seg.speaker}] {seg.text}" for seg in self._segments
        )
```

- [ ] **Step 4: Create `tests/fixtures/sample_segment.py`**

```python
from core.models import TranscriptSegment


def make_segment(
    text: str = "our NPI schedule starts in Q3",
    speaker: str = "client",
    language: str = "en",
) -> TranscriptSegment:
    return TranscriptSegment(speaker=speaker, text=text, language=language, timestamp=1000.0)
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
pytest tests/test_models.py -v
```

Expected: all 5 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add core/models.py tests/test_models.py tests/fixtures/sample_segment.py
git commit -m "feat: core data models (TranscriptSegment, PluginResult, ConversationContext)"
```

---

## Task 3: LLM Provider — OpenAI-Compatible

**Files:**
- Create: `llm/base.py`
- Create: `llm/openai_compat.py`
- Create: `tests/test_llm_openai_compat.py`

- [ ] **Step 1: Write failing tests**

Create `tests/test_llm_openai_compat.py`:

```python
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from llm.openai_compat import OpenAICompatProvider


@pytest.mark.asyncio
async def test_complete_returns_string():
    provider = OpenAICompatProvider(
        api_key="test-key",
        base_url="https://api.deepseek.com/v1",
        model="deepseek-chat",
    )
    mock_response = MagicMock()
    mock_response.choices = [MagicMock()]
    mock_response.choices[0].message.content = "NPI 是新产品导入流程"

    with patch.object(provider._client.chat.completions, "create", new=AsyncMock(return_value=mock_response)):
        result = await provider.complete(prompt="解释 NPI", system="你是一个助手")

    assert result == "NPI 是新产品导入流程"


@pytest.mark.asyncio
async def test_complete_with_empty_response():
    provider = OpenAICompatProvider(
        api_key="test-key",
        base_url="https://api.groq.com/openai/v1",
        model="llama3-70b-8192",
    )
    mock_response = MagicMock()
    mock_response.choices = [MagicMock()]
    mock_response.choices[0].message.content = ""

    with patch.object(provider._client.chat.completions, "create", new=AsyncMock(return_value=mock_response)):
        result = await provider.complete(prompt="test", system="test")

    assert result == ""
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_llm_openai_compat.py -v
```

Expected: `ImportError: No module named 'llm.openai_compat'`

- [ ] **Step 3: Create `llm/base.py`**

```python
from abc import ABC, abstractmethod
from typing import AsyncIterator


class LLMProvider(ABC):
    @abstractmethod
    async def complete(self, prompt: str, system: str) -> str:
        """Return a single completion string."""

    async def stream(self, prompt: str, system: str) -> AsyncIterator[str]:
        """Stream completion tokens. Default: yield complete() as one chunk."""
        result = await self.complete(prompt=prompt, system=system)
        yield result
```

- [ ] **Step 4: Create `llm/openai_compat.py`**

```python
from openai import AsyncOpenAI
from llm.base import LLMProvider


class OpenAICompatProvider(LLMProvider):
    def __init__(self, api_key: str, model: str, base_url: str | None = None):
        self._model = model
        self._client = AsyncOpenAI(api_key=api_key, base_url=base_url)

    async def complete(self, prompt: str, system: str) -> str:
        response = await self._client.chat.completions.create(
            model=self._model,
            messages=[
                {"role": "system", "content": system},
                {"role": "user", "content": prompt},
            ],
        )
        return response.choices[0].message.content or ""
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
pytest tests/test_llm_openai_compat.py -v
```

Expected: 2 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add llm/base.py llm/openai_compat.py tests/test_llm_openai_compat.py
git commit -m "feat: LLM provider abstraction + OpenAI-compatible implementation"
```

---

## Task 4: Plugin Base & Plugin Bus

**Files:**
- Create: `plugins/base.py`
- Create: `core/plugin_bus.py`
- Create: `tests/test_plugin_bus.py`

- [ ] **Step 1: Write failing tests**

Create `tests/test_plugin_bus.py`:

```python
import pytest
from unittest.mock import AsyncMock
from core.models import TranscriptSegment, PluginResult, ConversationContext
from core.plugin_bus import PluginBus
from plugins.base import AnalyzerPlugin
from tests.fixtures.sample_segment import make_segment


class EchoPlugin(AnalyzerPlugin):
    name = "echo"
    display_name = "Echo"
    ui_section = "terms"

    def should_trigger(self, segment: TranscriptSegment) -> bool:
        return True

    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult:
        return PluginResult(
            plugin_name=self.name,
            ui_section=self.ui_section,
            content=f"echo: {segment.text}",
        )


class NeverPlugin(AnalyzerPlugin):
    name = "never"
    display_name = "Never"
    ui_section = "terms"

    def should_trigger(self, segment: TranscriptSegment) -> bool:
        return False

    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult:
        raise AssertionError("should not be called")


@pytest.mark.asyncio
async def test_bus_calls_triggered_plugins():
    bus = PluginBus()
    bus.register(EchoPlugin())
    results = []
    segment = make_segment(text="hello world")
    async for result in bus.process(segment):
        results.append(result)
    assert len(results) == 1
    assert results[0].content == "echo: hello world"


@pytest.mark.asyncio
async def test_bus_skips_non_triggered_plugins():
    bus = PluginBus()
    bus.register(NeverPlugin())
    results = []
    async for result in bus.process(make_segment()):
        results.append(result)
    assert results == []


@pytest.mark.asyncio
async def test_bus_multiple_plugins():
    bus = PluginBus()
    bus.register(EchoPlugin())
    bus.register(EchoPlugin())  # two instances
    results = []
    async for result in bus.process(make_segment(text="test")):
        results.append(result)
    assert len(results) == 2


@pytest.mark.asyncio
async def test_bus_maintains_context():
    bus = PluginBus()
    bus.register(EchoPlugin())
    seg1 = make_segment(text="first")
    seg2 = make_segment(text="second")
    async for _ in bus.process(seg1):
        pass
    async for _ in bus.process(seg2):
        pass
    recent = bus.context.recent(2)
    assert recent[0].text == "first"
    assert recent[1].text == "second"
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_plugin_bus.py -v
```

Expected: `ImportError: No module named 'core.plugin_bus'`

- [ ] **Step 3: Create `plugins/base.py`**

```python
from abc import ABC, abstractmethod
from core.models import TranscriptSegment, PluginResult, ConversationContext


class AnalyzerPlugin(ABC):
    name: str
    display_name: str
    ui_section: str

    @abstractmethod
    def should_trigger(self, segment: TranscriptSegment) -> bool:
        """Return True if this plugin should process the given segment."""

    @abstractmethod
    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult:
        """Analyze segment and return a result to display in the UI."""
```

- [ ] **Step 4: Create `core/plugin_bus.py`**

```python
import asyncio
from typing import AsyncIterator
from core.models import TranscriptSegment, PluginResult, ConversationContext
from plugins.base import AnalyzerPlugin


class PluginBus:
    def __init__(self):
        self._plugins: list[AnalyzerPlugin] = []
        self.context = ConversationContext()

    def register(self, plugin: AnalyzerPlugin) -> None:
        self._plugins.append(plugin)

    async def process(self, segment: TranscriptSegment) -> AsyncIterator[PluginResult]:
        self.context.add(segment)
        triggered = [p for p in self._plugins if p.should_trigger(segment)]
        tasks = [p.analyze(segment, self.context) for p in triggered]
        for coro in asyncio.as_completed(tasks):
            result = await coro
            yield result
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
pytest tests/test_plugin_bus.py -v
```

Expected: 4 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add plugins/base.py core/plugin_bus.py tests/test_plugin_bus.py
git commit -m "feat: plugin base class and plugin bus"
```

---

## Task 5: Config Manager

**Files:**
- Create: `config/defaults.yaml`
- Create: `config/manager.py`
- Create: `tests/test_config_manager.py`

- [ ] **Step 1: Write failing tests**

Create `tests/test_config_manager.py`:

```python
import pytest
import tempfile
from pathlib import Path
from config.manager import ConfigManager


@pytest.fixture
def tmp_config_dir(tmp_path):
    return tmp_path / ".talksage"


def test_loads_defaults_when_no_file(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("transcribe.mode") == "local"
    assert mgr.get("transcribe.model") == "base"


def test_creates_config_dir_on_save(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    mgr.save()
    assert (tmp_config_dir / "config.yaml").exists()


def test_get_nested_key(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("plugins.term_explainer.enabled") is True


def test_set_and_get(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    mgr.set("transcribe.mode", "api")
    assert mgr.get("transcribe.mode") == "api"


def test_get_missing_key_returns_default(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("nonexistent.key", default="fallback") == "fallback"


def test_get_llm_provider_config(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    provider = mgr.get_llm_provider("deepseek")
    assert provider["model"] == "deepseek-chat"
    assert "base_url" in provider
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_config_manager.py -v
```

Expected: `ImportError: No module named 'config.manager'`

- [ ] **Step 3: Create `config/defaults.yaml`**

```yaml
transcribe:
  mode: local         # local / api
  model: base         # tiny, base, small, medium, large

llm:
  default: deepseek
  providers:
    claude:
      base_url: null
      api_key: ""
      model: claude-sonnet-4-6
    deepseek:
      base_url: https://api.deepseek.com/v1
      api_key: ""
      model: deepseek-chat
    kimi:
      base_url: https://api.moonshot.cn/v1
      api_key: ""
      model: moonshot-v1-32k
    minimax:
      base_url: https://api.minimax.chat/v1
      api_key: ""
      model: abab6.5s-chat
    groq:
      base_url: https://api.groq.com/openai/v1
      api_key: ""
      model: llama3-70b-8192
    ollama:
      base_url: http://localhost:11434/v1
      api_key: ollama
      model: llama3

plugins:
  term_explainer:
    enabled: true
    llm: deepseek
  translator:
    enabled: false
  tech_evaluator:
    enabled: false
  negotiation_analyzer:
    enabled: false
```

- [ ] **Step 4: Create `config/manager.py`**

```python
from pathlib import Path
from typing import Any
import yaml


_DEFAULTS_PATH = Path(__file__).parent / "defaults.yaml"


def _deep_merge(base: dict, override: dict) -> dict:
    result = base.copy()
    for key, value in override.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = _deep_merge(result[key], value)
        else:
            result[key] = value
    return result


class ConfigManager:
    def __init__(self, config_dir: Path | None = None):
        self._dir = config_dir or Path.home() / ".talksage"
        self._path = self._dir / "config.yaml"
        with open(_DEFAULTS_PATH) as f:
            self._data: dict = yaml.safe_load(f)
        if self._path.exists():
            with open(self._path) as f:
                user_data = yaml.safe_load(f) or {}
            self._data = _deep_merge(self._data, user_data)

    def get(self, key: str, default: Any = None) -> Any:
        parts = key.split(".")
        node = self._data
        for part in parts:
            if not isinstance(node, dict) or part not in node:
                return default
            node = node[part]
        return node

    def set(self, key: str, value: Any) -> None:
        parts = key.split(".")
        node = self._data
        for part in parts[:-1]:
            node = node.setdefault(part, {})
        node[parts[-1]] = value

    def save(self) -> None:
        self._dir.mkdir(parents=True, exist_ok=True)
        with open(self._path, "w") as f:
            yaml.dump(self._data, f, allow_unicode=True, default_flow_style=False)

    def get_llm_provider(self, name: str) -> dict:
        return self.get(f"llm.providers.{name}", default={})
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
pytest tests/test_config_manager.py -v
```

Expected: 6 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add config/defaults.yaml config/manager.py tests/test_config_manager.py
git commit -m "feat: config manager with defaults and deep merge"
```

---

## Task 6: Term Explainer Plugin

**Files:**
- Create: `plugins/term_explainer.py`
- Create: `tests/test_term_explainer.py`

- [ ] **Step 1: Write failing tests**

Create `tests/test_term_explainer.py`:

```python
import pytest
from unittest.mock import AsyncMock
from core.models import ConversationContext
from plugins.term_explainer import TermExplainerPlugin
from llm.base import LLMProvider
from tests.fixtures.sample_segment import make_segment


class MockLLM(LLMProvider):
    def __init__(self, response: str):
        self._response = response

    async def complete(self, prompt: str, system: str) -> str:
        return self._response


def test_should_trigger_on_english_with_acronyms():
    plugin = TermExplainerPlugin(llm=MockLLM(""))
    seg = make_segment(text="our NPI schedule starts Q3", language="en", speaker="client")
    assert plugin.should_trigger(seg) is True


def test_should_not_trigger_on_chinese():
    plugin = TermExplainerPlugin(llm=MockLLM(""))
    seg = make_segment(text="我们下周开会", language="zh", speaker="client")
    assert plugin.should_trigger(seg) is False


def test_should_not_trigger_on_user_speech():
    plugin = TermExplainerPlugin(llm=MockLLM(""))
    seg = make_segment(text="NPI is important", language="en", speaker="user")
    assert plugin.should_trigger(seg) is False


def test_should_not_trigger_without_acronyms():
    plugin = TermExplainerPlugin(llm=MockLLM(""))
    seg = make_segment(text="the meeting starts at three", language="en", speaker="client")
    assert plugin.should_trigger(seg) is False


@pytest.mark.asyncio
async def test_analyze_returns_plugin_result():
    plugin = TermExplainerPlugin(llm=MockLLM("NPI = 新产品导入流程 (New Product Introduction)"))
    seg = make_segment(text="our NPI schedule starts Q3", language="en", speaker="client")
    ctx = ConversationContext()
    result = await plugin.analyze(seg, ctx)
    assert result.plugin_name == "term_explainer"
    assert result.ui_section == "terms"
    assert "NPI" in result.content


@pytest.mark.asyncio
async def test_analyze_prompt_contains_segment_text():
    captured_prompts = []

    class CapturingLLM(LLMProvider):
        async def complete(self, prompt: str, system: str) -> str:
            captured_prompts.append(prompt)
            return "BOQ = 物料清单"

    plugin = TermExplainerPlugin(llm=CapturingLLM())
    seg = make_segment(text="please check the BOQ first", language="en", speaker="client")
    ctx = ConversationContext()
    await plugin.analyze(seg, ctx)
    assert "BOQ" in captured_prompts[0]
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_term_explainer.py -v
```

Expected: `ImportError: No module named 'plugins.term_explainer'`

- [ ] **Step 3: Create `plugins/term_explainer.py`**

```python
import re
from core.models import TranscriptSegment, PluginResult, ConversationContext
from plugins.base import AnalyzerPlugin
from llm.base import LLMProvider

# Match sequences of 2+ uppercase letters (acronyms like NPI, BOQ, MOQ, ETA, etc.)
_ACRONYM_RE = re.compile(r'\b[A-Z]{2,}\b')

_SYSTEM_PROMPT = (
    "你是一位硬件制造业和商务谈判领域的专家助手。"
    "用户正在和英文客户谈话，请帮助解释对话中出现的专业术语和缩写。"
    "回答要简洁，使用中文，格式：缩写 = 中文全称（英文全称），然后一句话说明含义。"
    "如果有多个术语，每个术语单独一行。"
)


class TermExplainerPlugin(AnalyzerPlugin):
    name = "term_explainer"
    display_name = "术语解释"
    ui_section = "terms"

    def __init__(self, llm: LLMProvider):
        self._llm = llm

    def should_trigger(self, segment: TranscriptSegment) -> bool:
        if segment.speaker != "client":
            return False
        if segment.language != "en":
            return False
        return bool(_ACRONYM_RE.search(segment.text))

    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult:
        acronyms = _ACRONYM_RE.findall(segment.text)
        prompt = (
            f"客户说：\"{segment.text}\"\n\n"
            f"请解释其中出现的术语/缩写：{', '.join(set(acronyms))}"
        )
        content = await self._llm.complete(prompt=prompt, system=_SYSTEM_PROMPT)
        return PluginResult(
            plugin_name=self.name,
            ui_section=self.ui_section,
            content=content,
            priority=1,
        )
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
pytest tests/test_term_explainer.py -v
```

Expected: 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add plugins/term_explainer.py tests/test_term_explainer.py
git commit -m "feat: term explainer plugin (acronym detection + Chinese explanation)"
```

---

## Task 7: Transcribe Engine (Local Whisper)

**Files:**
- Create: `core/transcribe_engine.py`
- Create: `tests/fixtures/silence.wav` (generated in test setup)

> Note: `faster-whisper` wraps CTranslate2 and downloads model weights on first use (~150MB for "base"). Tests mock the whisper model to avoid network calls.

- [ ] **Step 1: Write failing tests**

Create `tests/test_transcribe_engine.py`:

```python
import pytest
import numpy as np
from unittest.mock import MagicMock, patch
from core.transcribe_engine import TranscribeEngine
from core.models import TranscriptSegment


@pytest.fixture
def mock_whisper_model():
    mock = MagicMock()
    segment = MagicMock()
    segment.text = " our NPI schedule starts in Q3"
    mock.transcribe.return_value = ([segment], MagicMock(language="en"))
    return mock


def test_transcribe_returns_segment(mock_whisper_model):
    engine = TranscribeEngine(model_size="base")
    engine._model = mock_whisper_model

    audio = np.zeros(16000, dtype=np.float32)
    result = engine.transcribe(audio, speaker="client")

    assert isinstance(result, TranscriptSegment)
    assert result.speaker == "client"
    assert result.language == "en"
    assert "NPI" in result.text


def test_transcribe_strips_leading_whitespace(mock_whisper_model):
    engine = TranscribeEngine(model_size="base")
    engine._model = mock_whisper_model

    audio = np.zeros(16000, dtype=np.float32)
    result = engine.transcribe(audio, speaker="client")
    assert not result.text.startswith(" ")


def test_transcribe_returns_none_for_silence():
    engine = TranscribeEngine(model_size="base")
    mock = MagicMock()
    mock.transcribe.return_value = ([], MagicMock(language="en"))
    engine._model = mock

    audio = np.zeros(16000, dtype=np.float32)
    result = engine.transcribe(audio, speaker="client")
    assert result is None


def test_engine_lazy_loads_model():
    engine = TranscribeEngine(model_size="base")
    assert engine._model is None  # not loaded until first use
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_transcribe_engine.py -v
```

Expected: `ImportError: No module named 'core.transcribe_engine'`

- [ ] **Step 3: Create `core/transcribe_engine.py`**

```python
import numpy as np
from core.models import TranscriptSegment


class TranscribeEngine:
    def __init__(self, model_size: str = "base"):
        self._model_size = model_size
        self._model = None  # lazy load on first use

    def _load_model(self):
        from faster_whisper import WhisperModel
        self._model = WhisperModel(self._model_size, device="cpu", compute_type="int8")

    def transcribe(self, audio: "np.ndarray", speaker: str) -> TranscriptSegment | None:
        if self._model is None:
            self._load_model()
        segments, info = self._model.transcribe(audio, beam_size=5)
        text_parts = [seg.text for seg in segments]
        if not text_parts:
            return None
        text = "".join(text_parts).strip()
        if not text:
            return None
        return TranscriptSegment(
            speaker=speaker,
            text=text,
            language=info.language,
        )
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
pytest tests/test_transcribe_engine.py -v
```

Expected: 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add core/transcribe_engine.py tests/test_transcribe_engine.py
git commit -m "feat: transcribe engine with lazy-loaded local Whisper"
```

---

## Task 8: Audio Hub (Microphone Capture)

**Files:**
- Create: `core/audio_hub.py`

> Note: `sounddevice` requires a real audio device. Tests mock the callback mechanism entirely — no actual audio hardware needed.

- [ ] **Step 1: Write failing tests**

Create `tests/test_audio_hub.py`:

```python
import pytest
import numpy as np
from unittest.mock import MagicMock, patch
from core.audio_hub import AudioHub


def test_audio_hub_initial_state():
    hub = AudioHub(sample_rate=16000, chunk_seconds=2)
    assert not hub.is_recording


def test_audio_hub_accumulates_audio():
    hub = AudioHub(sample_rate=16000, chunk_seconds=2)
    chunk = np.zeros((1600, 1), dtype=np.float32)
    hub._on_mic_data(chunk, None, None, None)
    hub._on_mic_data(chunk, None, None, None)
    assert hub._mic_buffer.shape[0] == 3200


def test_audio_hub_flushes_when_full():
    flushed = []
    hub = AudioHub(sample_rate=16000, chunk_seconds=1)
    hub.on_segment = lambda audio, speaker: flushed.append((audio, speaker))

    # Feed exactly 1 second of audio (16000 samples)
    chunk = np.zeros((16000, 1), dtype=np.float32)
    hub._on_mic_data(chunk, None, None, None)

    assert len(flushed) == 1
    assert flushed[0][1] == "user"
    assert flushed[0][0].shape[0] == 16000


def test_audio_hub_resets_buffer_after_flush():
    hub = AudioHub(sample_rate=16000, chunk_seconds=1)
    hub.on_segment = lambda audio, speaker: None
    chunk = np.zeros((16000, 1), dtype=np.float32)
    hub._on_mic_data(chunk, None, None, None)
    assert hub._mic_buffer.shape[0] == 0
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_audio_hub.py -v
```

Expected: `ImportError: No module named 'core.audio_hub'`

- [ ] **Step 3: Create `core/audio_hub.py`**

```python
import numpy as np
from typing import Callable


class AudioHub:
    def __init__(self, sample_rate: int = 16000, chunk_seconds: int = 3):
        self._sample_rate = sample_rate
        self._chunk_size = sample_rate * chunk_seconds
        self._mic_buffer = np.empty((0,), dtype=np.float32)
        self._stream = None
        self.is_recording = False
        # Callback: (audio: np.ndarray, speaker: str) -> None
        self.on_segment: Callable[[np.ndarray, str], None] | None = None

    def start(self) -> None:
        import sounddevice as sd
        self._stream = sd.InputStream(
            samplerate=self._sample_rate,
            channels=1,
            dtype="float32",
            callback=self._on_mic_data,
        )
        self._stream.start()
        self.is_recording = True

    def stop(self) -> None:
        if self._stream:
            self._stream.stop()
            self._stream.close()
            self._stream = None
        self.is_recording = False

    def _on_mic_data(self, indata: np.ndarray, frames, time, status) -> None:
        flat = indata[:, 0]
        self._mic_buffer = np.concatenate([self._mic_buffer, flat])
        if len(self._mic_buffer) >= self._chunk_size:
            segment = self._mic_buffer[: self._chunk_size].copy()
            self._mic_buffer = self._mic_buffer[self._chunk_size :]
            if self.on_segment:
                self.on_segment(segment, "user")
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
pytest tests/test_audio_hub.py -v
```

Expected: 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add core/audio_hub.py tests/test_audio_hub.py
git commit -m "feat: audio hub with microphone capture and segment flush"
```

---

## Task 9: PySide6 Sidebar UI

**Files:**
- Create: `ui/sections/transcript.py`
- Create: `ui/sections/terms.py`
- Create: `ui/main_window.py`

> UI tests use `pytest-qt`. They test widget state, not visual rendering.

- [ ] **Step 1: Write failing tests**

Create `tests/test_ui.py`:

```python
import pytest
from pytestqt.qtbot import QtBot
from PySide6.QtWidgets import QApplication
from ui.sections.terms import TermsSection
from ui.sections.transcript import TranscriptSection
from core.models import PluginResult, TranscriptSegment


@pytest.fixture(scope="session")
def qapp():
    return QApplication.instance() or QApplication([])


def test_terms_section_adds_result(qapp, qtbot):
    section = TermsSection()
    qtbot.addWidget(section)
    result = PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = 新产品导入流程",
    )
    section.add_result(result)
    assert section.count() == 1


def test_terms_section_shows_content(qapp, qtbot):
    section = TermsSection()
    qtbot.addWidget(section)
    result = PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="BOQ = 物料清单",
    )
    section.add_result(result)
    item_text = section.item(0).text()
    assert "BOQ" in item_text


def test_transcript_section_adds_segment(qapp, qtbot):
    section = TranscriptSection()
    qtbot.addWidget(section)
    seg = TranscriptSegment(speaker="client", text="hello world", language="en")
    section.add_segment(seg)
    assert section.document().toPlainText() != ""


def test_transcript_section_shows_speaker(qapp, qtbot):
    section = TranscriptSection()
    qtbot.addWidget(section)
    seg = TranscriptSegment(speaker="client", text="our NPI schedule", language="en")
    section.add_segment(seg)
    text = section.document().toPlainText()
    assert "client" in text.lower()
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_ui.py -v
```

Expected: `ImportError: No module named 'ui.sections.terms'`

- [ ] **Step 3: Create `ui/sections/terms.py`**

```python
from PySide6.QtWidgets import QListWidget, QListWidgetItem
from PySide6.QtCore import Slot
from core.models import PluginResult


class TermsSection(QListWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMaximumHeight(200)

    @Slot(object)
    def add_result(self, result: PluginResult) -> None:
        item = QListWidgetItem(result.content)
        self.insertItem(0, item)  # newest at top
        if self.count() > 20:
            self.takeItem(self.count() - 1)
```

- [ ] **Step 4: Create `ui/sections/transcript.py`**

```python
from PySide6.QtWidgets import QTextEdit
from PySide6.QtCore import Slot
from core.models import TranscriptSegment


class TranscriptSection(QTextEdit):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setReadOnly(True)
        self.setMaximumHeight(150)

    @Slot(object)
    def add_segment(self, segment: TranscriptSegment) -> None:
        label = f"[{segment.speaker}]"
        self.append(f"{label} {segment.text}")
        self.ensureCursorVisible()
```

- [ ] **Step 5: Create `ui/main_window.py`**

```python
from PySide6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QLabel, QPushButton, QGroupBox
)
from PySide6.QtCore import Qt, Signal, Slot
from core.models import TranscriptSegment, PluginResult
from ui.sections.transcript import TranscriptSection
from ui.sections.terms import TermsSection


class MainWindow(QMainWindow):
    # Signals used to safely update UI from background threads
    segment_received = Signal(object)
    result_received = Signal(object)

    def __init__(self):
        super().__init__()
        self.setWindowTitle("TalkSage")
        self.setMinimumWidth(320)
        self.setMaximumWidth(480)

        central = QWidget()
        self.setCentralWidget(central)
        layout = QVBoxLayout(central)

        # Record toggle button
        self._record_btn = QPushButton("▶ 开始监听")
        self._record_btn.setCheckable(True)
        self._record_btn.toggled.connect(self._on_record_toggled)
        layout.addWidget(self._record_btn)

        # Transcript section
        transcript_box = QGroupBox("🎙 实时转写")
        transcript_layout = QVBoxLayout(transcript_box)
        self.transcript_section = TranscriptSection()
        transcript_layout.addWidget(self.transcript_section)
        layout.addWidget(transcript_box)

        # Terms section
        terms_box = QGroupBox("📖 术语")
        terms_layout = QVBoxLayout(terms_box)
        self.terms_section = TermsSection()
        terms_layout.addWidget(self.terms_section)
        layout.addWidget(terms_box)

        layout.addStretch()

        # Connect signals to UI slots
        self.segment_received.connect(self.transcript_section.add_segment)
        self.result_received.connect(self._route_result)

    @Slot(bool)
    def _on_record_toggled(self, checked: bool) -> None:
        self._record_btn.setText("⏹ 停止监听" if checked else "▶ 开始监听")

    @Slot(object)
    def _route_result(self, result: PluginResult) -> None:
        if result.ui_section == "terms":
            self.terms_section.add_result(result)
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
pytest tests/test_ui.py -v
```

Expected: 4 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add ui/sections/terms.py ui/sections/transcript.py ui/main_window.py tests/test_ui.py
git commit -m "feat: PySide6 sidebar with transcript and terms sections"
```

---

## Task 10: Wire Everything Together

**Files:**
- Modify: `main.py`
- Create: `core/pipeline.py`

- [ ] **Step 1: Write failing test**

Create `tests/test_pipeline.py`:

```python
import pytest
import numpy as np
from unittest.mock import MagicMock, AsyncMock, patch
from core.pipeline import Pipeline
from core.models import PluginResult


@pytest.mark.asyncio
async def test_pipeline_processes_audio_segment():
    results = []

    pipeline = Pipeline.__new__(Pipeline)
    pipeline._bus = MagicMock()

    async def fake_process(seg):
        result = PluginResult(plugin_name="term_explainer", ui_section="terms", content="NPI = 新产品导入")
        yield result

    pipeline._bus.process = fake_process

    mock_engine = MagicMock()
    from core.models import TranscriptSegment
    mock_engine.transcribe.return_value = TranscriptSegment(
        speaker="client", text="NPI schedule", language="en"
    )
    pipeline._engine = mock_engine
    pipeline.on_segment = None
    pipeline.on_result = lambda r: results.append(r)

    import asyncio
    await pipeline._handle_audio(np.zeros(16000, dtype=np.float32), "client")
    assert len(results) == 1
    assert "NPI" in results[0].content
```

- [ ] **Step 2: Run test to verify it fails**

```bash
pytest tests/test_pipeline.py -v
```

Expected: `ImportError: No module named 'core.pipeline'`

- [ ] **Step 3: Create `core/pipeline.py`**

```python
import asyncio
import numpy as np
from typing import Callable
from core.audio_hub import AudioHub
from core.transcribe_engine import TranscribeEngine
from core.plugin_bus import PluginBus
from core.models import TranscriptSegment, PluginResult


class Pipeline:
    def __init__(
        self,
        hub: AudioHub,
        engine: TranscribeEngine,
        bus: PluginBus,
    ):
        self._hub = hub
        self._engine = engine
        self._bus = bus
        self._loop: asyncio.AbstractEventLoop | None = None
        self.on_segment: Callable[[TranscriptSegment], None] | None = None
        self.on_result: Callable[[PluginResult], None] | None = None
        self._hub.on_segment = self._schedule_audio

    def start(self) -> None:
        self._loop = asyncio.new_event_loop()
        import threading
        t = threading.Thread(target=self._loop.run_forever, daemon=True)
        t.start()
        self._hub.start()

    def stop(self) -> None:
        self._hub.stop()
        if self._loop:
            self._loop.call_soon_threadsafe(self._loop.stop)

    def _schedule_audio(self, audio: np.ndarray, speaker: str) -> None:
        if self._loop:
            asyncio.run_coroutine_threadsafe(
                self._handle_audio(audio, speaker), self._loop
            )

    async def _handle_audio(self, audio: np.ndarray, speaker: str) -> None:
        segment = self._engine.transcribe(audio, speaker=speaker)
        if segment is None:
            return
        if self.on_segment:
            self.on_segment(segment)
        async for result in self._bus.process(segment):
            if self.on_result:
                self.on_result(result)
```

- [ ] **Step 4: Update `main.py` to wire pipeline to UI**

```python
import sys
from PySide6.QtWidgets import QApplication
from ui.main_window import MainWindow
from core.audio_hub import AudioHub
from core.transcribe_engine import TranscribeEngine
from core.plugin_bus import PluginBus
from core.pipeline import Pipeline
from plugins.term_explainer import TermExplainerPlugin
from llm.openai_compat import OpenAICompatProvider
from config.manager import ConfigManager


def build_pipeline(config: ConfigManager) -> Pipeline:
    hub = AudioHub()
    engine = TranscribeEngine(model_size=config.get("transcribe.model", "base"))
    bus = PluginBus()

    if config.get("plugins.term_explainer.enabled"):
        provider_name = config.get("plugins.term_explainer.llm", "deepseek")
        provider_cfg = config.get_llm_provider(provider_name)
        llm = OpenAICompatProvider(
            api_key=provider_cfg.get("api_key", ""),
            model=provider_cfg.get("model", "deepseek-chat"),
            base_url=provider_cfg.get("base_url"),
        )
        bus.register(TermExplainerPlugin(llm=llm))

    return Pipeline(hub=hub, engine=engine, bus=bus)


def main():
    app = QApplication(sys.argv)
    config = ConfigManager()
    window = MainWindow()

    pipeline = build_pipeline(config)
    pipeline.on_segment = lambda seg: window.segment_received.emit(seg)
    pipeline.on_result = lambda res: window.result_received.emit(res)

    window._record_btn.toggled.connect(
        lambda checked: pipeline.start() if checked else pipeline.stop()
    )

    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
```

- [ ] **Step 5: Run all tests**

```bash
pytest tests/ -v
```

Expected: all tests PASS.

- [ ] **Step 6: Run the app manually to verify**

```bash
python main.py
```

Expected:
- Sidebar window opens
- Click "▶ 开始监听" button — button text changes to "⏹ 停止监听"
- Speak into mic — transcript section shows `[user] <your words>`
- English acronyms (say "NPI", "BOQ") should trigger term explainer (requires valid API key in `~/.talksage/config.yaml`)

- [ ] **Step 7: Final commit**

```bash
git add core/pipeline.py tests/test_pipeline.py main.py
git commit -m "feat: wire pipeline to UI — Phase 1 MVP complete"
```

---

## Running All Tests

```bash
pytest tests/ -v --tb=short
```

Expected: 25+ tests, all PASS.

## Phase 1 Complete Checklist

- [ ] Mic audio captured in 3-second chunks
- [ ] Local Whisper transcribes audio to text with speaker label
- [ ] English segments with acronyms trigger term explainer plugin
- [ ] Term explainer calls LLM and returns Chinese explanation
- [ ] Result appears in sidebar terms section
- [ ] Transcript section shows running conversation
- [ ] Record button starts/stops pipeline
- [ ] Config loads from `~/.talksage/config.yaml` (or defaults)
