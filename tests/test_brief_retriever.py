import pytest
from pathlib import Path
from core.knowledge_base import KnowledgeBase
from core.models import ConversationContext
from plugins.brief_retriever import BriefRetrieverPlugin
from tests.fixtures.sample_segment import make_segment


def test_should_trigger_on_client_with_kb_hit(tmp_path):
    folder = tmp_path / "kb"
    folder.mkdir()
    (folder / "a.md").write_text("Customer MOQ is 500 units for connectors.\n", encoding="utf-8")
    kb = KnowledgeBase()
    kb.index_folder(folder)
    plugin = BriefRetrieverPlugin(kb=kb, cooldown_seconds=0)
    seg = make_segment(text="What is your MOQ?", language="en", speaker="client")
    assert plugin.should_trigger(seg) is True


def test_should_not_trigger_without_hits(tmp_path):
    kb = KnowledgeBase()
    plugin = BriefRetrieverPlugin(kb=kb, cooldown_seconds=0)
    seg = make_segment(text="Hello there", language="en", speaker="client")
    assert plugin.should_trigger(seg) is False


@pytest.mark.asyncio
async def test_analyze_returns_suggestions_section(tmp_path):
    folder = tmp_path / "kb"
    folder.mkdir()
    (folder / "a.md").write_text("# Pricing\n\nMOQ is 500 units.\n", encoding="utf-8")
    kb = KnowledgeBase()
    kb.index_folder(folder)
    plugin = BriefRetrieverPlugin(kb=kb, cooldown_seconds=0)
    seg = make_segment(text="Please confirm the MOQ", language="en", speaker="client")
    result = await plugin.analyze(seg, ConversationContext())
    assert result.ui_section == "suggestions"
    assert "MOQ" in result.content or "500" in result.content
