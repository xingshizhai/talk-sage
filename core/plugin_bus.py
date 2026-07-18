import asyncio
from typing import AsyncGenerator
from core.models import TranscriptSegment, PluginResult, ConversationContext
from core.conversation_state import StateTracker
from plugins.base import AnalyzerPlugin

_SENTINEL = object()


class PluginBus:
    def __init__(self):
        self._plugins: list[AnalyzerPlugin] = []
        self.context = ConversationContext()
        self._state_tracker = StateTracker()
        # Keep context.state as the same object the tracker mutates
        self.context.state = self._state_tracker.state

    def register(self, plugin: AnalyzerPlugin) -> None:
        self._plugins.append(plugin)

    async def process(self, segment: TranscriptSegment) -> AsyncGenerator[PluginResult, None]:
        self.context.add(segment)
        self._state_tracker.update(segment)
        self.context.state = self._state_tracker.state

        triggered = [p for p in self._plugins if p.should_trigger(segment)]
        if not triggered:
            return

        queue: asyncio.Queue = asyncio.Queue()

        async def run_plugin(plugin: AnalyzerPlugin) -> None:
            try:
                async for result in plugin.analyze_stream(segment, self.context):
                    await queue.put(result)
            finally:
                await queue.put(_SENTINEL)

        tasks = [asyncio.create_task(run_plugin(p)) for p in triggered]
        finished = 0
        while finished < len(tasks):
            item = await queue.get()
            if item is _SENTINEL:
                finished += 1
            else:
                yield item
        await asyncio.gather(*tasks)
