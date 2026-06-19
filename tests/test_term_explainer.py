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
