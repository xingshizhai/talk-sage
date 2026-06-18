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
