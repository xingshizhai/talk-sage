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


def test_should_not_trigger_on_already_seen_acronym():
    plugin = TermExplainerPlugin(llm=MockLLM(""), cooldown_seconds=0)
    seg = make_segment(text="our NPI schedule starts Q3", language="en", speaker="client")
    assert plugin.should_trigger(seg) is True
    # Simulate analyze having marked NPI as seen
    plugin.mark_seen(["NPI"])
    assert plugin.should_trigger(seg) is False


def test_should_trigger_on_new_acronym_even_if_others_seen():
    plugin = TermExplainerPlugin(llm=MockLLM(""), cooldown_seconds=0)
    plugin.mark_seen(["NPI"])
    seg = make_segment(text="NPI and BOQ are ready", language="en", speaker="client")
    assert plugin.should_trigger(seg) is True


def test_should_not_trigger_during_cooldown():
    plugin = TermExplainerPlugin(llm=MockLLM(""), cooldown_seconds=60)
    plugin.mark_seen([])  # no-op
    plugin._last_trigger_at = __import__("time").time()
    seg = make_segment(text="please review the RFQ", language="en", speaker="client")
    assert plugin.should_trigger(seg) is False


@pytest.mark.asyncio
async def test_analyze_only_requests_unseen_acronyms():
    captured_prompts = []

    class CapturingLLM(LLMProvider):
        async def complete(self, prompt: str, system: str) -> str:
            captured_prompts.append(prompt)
            return "BOQ = 物料清单"

    plugin = TermExplainerPlugin(llm=CapturingLLM(), cooldown_seconds=0)
    plugin.mark_seen(["NPI"])
    seg = make_segment(text="NPI and BOQ are ready", language="en", speaker="client")
    ctx = ConversationContext()
    await plugin.analyze(seg, ctx)
    assert "BOQ" in captured_prompts[0]
    assert "NPI" not in captured_prompts[0].split("术语/缩写：")[-1]


@pytest.mark.asyncio
async def test_analyze_marks_acronyms_seen():
    plugin = TermExplainerPlugin(llm=MockLLM("MOQ = 最小起订量"), cooldown_seconds=0)
    seg = make_segment(text="MOQ is 500 units", language="en", speaker="client")
    await plugin.analyze(seg, ConversationContext())
    assert plugin.should_trigger(seg) is False


@pytest.mark.asyncio
async def test_analyze_stream_yields_skeleton_then_final():
    plugin = TermExplainerPlugin(llm=MockLLM("NPI = 新产品导入"), cooldown_seconds=0)
    seg = make_segment(text="our NPI schedule", language="en", speaker="client")
    results = []
    async for r in plugin.analyze_stream(seg, ConversationContext()):
        results.append(r)
    assert len(results) == 2
    assert results[0].status == "skeleton"
    assert "NPI" in results[0].content
    assert results[1].status == "final"
    assert "新产品导入" in results[1].content
    assert results[0].result_id == results[1].result_id
    assert results[0].result_id != ""
