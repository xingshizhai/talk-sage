import asyncio
from typing import AsyncGenerator
from core.models import TranscriptSegment, PluginResult, ConversationContext
from plugins.base import AnalyzerPlugin


class PluginBus:
    def __init__(self):
        self._plugins: list[AnalyzerPlugin] = []
        self.context = ConversationContext()

    def register(self, plugin: AnalyzerPlugin) -> None:
        self._plugins.append(plugin)

    async def process(self, segment: TranscriptSegment) -> AsyncGenerator[PluginResult, None]:
        self.context.add(segment)
        triggered = [p for p in self._plugins if p.should_trigger(segment)]
        tasks = [p.analyze(segment, self.context) for p in triggered]
        for coro in asyncio.as_completed(tasks):
            result = await coro
            yield result
