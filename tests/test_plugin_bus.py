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
