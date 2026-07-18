from abc import ABC, abstractmethod
from typing import AsyncGenerator, ClassVar
from core.models import TranscriptSegment, PluginResult, ConversationContext


class AnalyzerPlugin(ABC):
    name: ClassVar[str]
    display_name: ClassVar[str]
    ui_section: ClassVar[str]

    @abstractmethod
    def should_trigger(self, segment: TranscriptSegment) -> bool:
        """Return True if this plugin should process the given segment."""

    @abstractmethod
    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult:
        """Analyze segment and return a final result to display in the UI."""

    async def analyze_stream(
        self, segment: TranscriptSegment, context: ConversationContext
    ) -> AsyncGenerator[PluginResult, None]:
        """Yield progressive results (skeleton → final). Default: single final result."""
        yield await self.analyze(segment, context)
