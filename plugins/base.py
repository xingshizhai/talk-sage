from abc import ABC, abstractmethod
from typing import ClassVar
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
        """Analyze segment and return a result to display in the UI."""
