import time
from core.models import TranscriptSegment, PluginResult, ConversationContext
from core.knowledge_base import KnowledgeBase
from plugins.base import AnalyzerPlugin


class BriefRetrieverPlugin(AnalyzerPlugin):
    """Surface relevant customer-brief snippets from the local knowledge base."""

    name = "brief_retriever"
    display_name = "客户简报"
    ui_section = "suggestions"

    def __init__(
        self,
        kb: KnowledgeBase,
        cooldown_seconds: float = 15.0,
        min_score: float = 0.08,
    ):
        self._kb = kb
        self._cooldown_seconds = cooldown_seconds
        self._min_score = min_score
        self._last_trigger_at = 0.0

    def should_trigger(self, segment: TranscriptSegment) -> bool:
        if segment.speaker != "client":
            return False
        if self._kb.chunk_count == 0:
            return False
        if self._cooldown_seconds > 0 and self._last_trigger_at > 0:
            if time.time() - self._last_trigger_at < self._cooldown_seconds:
                return False
        hits = self._kb.search(segment.text, top_k=1, min_score=self._min_score)
        return bool(hits)

    async def analyze(
        self, segment: TranscriptSegment, context: ConversationContext
    ) -> PluginResult:
        hits = self._kb.search(segment.text, top_k=2, min_score=self._min_score)
        self._last_trigger_at = time.time()
        if not hits:
            return PluginResult(
                plugin_name=self.name,
                ui_section=self.ui_section,
                content="",
                priority=0,
            )
        lines = []
        for h in hits:
            label = h.heading or h.source
            lines.append(f"[{label}] {h.text.strip()[:280]}")
        return PluginResult(
            plugin_name=self.name,
            ui_section=self.ui_section,
            content="\n\n".join(lines),
            priority=2,
        )
