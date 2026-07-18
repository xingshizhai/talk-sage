import pytest
from unittest.mock import MagicMock
from core.plugin_bus import PluginBus
from core.models import TranscriptSegment, PluginResult, ConversationContext
from plugins.base import AnalyzerPlugin


class StreamingPlugin(AnalyzerPlugin):
    name = "stream_test"
    display_name = "Stream Test"
    ui_section = "terms"

    def should_trigger(self, segment: TranscriptSegment) -> bool:
        return True

    async def analyze(self, segment, context):
        return PluginResult(plugin_name=self.name, ui_section="terms", content="final")

    async def analyze_stream(self, segment, context):
        yield PluginResult(
            plugin_name=self.name,
            ui_section="terms",
            content="NPI = …",
            result_id="r1",
            status="skeleton",
        )
        yield PluginResult(
            plugin_name=self.name,
            ui_section="terms",
            content="NPI = done",
            result_id="r1",
            status="final",
        )


@pytest.mark.asyncio
async def test_plugin_bus_yields_progressive_results():
    bus = PluginBus()
    bus.register(StreamingPlugin())
    seg = TranscriptSegment(speaker="client", text="NPI", language="en")
    results = [r async for r in bus.process(seg)]
    assert len(results) == 2
    assert results[0].status == "skeleton"
    assert results[1].status == "final"
