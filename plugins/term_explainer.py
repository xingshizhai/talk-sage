import re
import time
import uuid
from typing import AsyncGenerator
from core.models import TranscriptSegment, PluginResult, ConversationContext
from plugins.base import AnalyzerPlugin
from llm.base import LLMProvider

# Match sequences of 2+ uppercase letters (acronyms like NPI, BOQ, MOQ, ETA, etc.)
_ACRONYM_RE = re.compile(r'\b[A-Z]{2,}\b')

_SYSTEM_PROMPT = (
    "你是一位硬件制造业和商务谈判领域的专家助手。"
    "用户正在和英文客户谈话，请帮助解释对话中出现的专业术语和缩写。"
    "回答要简洁，使用中文，格式：缩写 = 中文全称（英文全称），然后一句话说明含义。"
    "如果有多个术语，每个术语单独一行。"
)

_DEFAULT_COOLDOWN_SECONDS = 10.0


class TermExplainerPlugin(AnalyzerPlugin):
    name = "term_explainer"
    display_name = "术语解释"
    ui_section = "terms"

    def __init__(self, llm: LLMProvider, cooldown_seconds: float = _DEFAULT_COOLDOWN_SECONDS):
        self._llm = llm
        self._cooldown_seconds = cooldown_seconds
        self._seen: set[str] = set()
        self._last_trigger_at: float = 0.0

    def mark_seen(self, acronyms: list[str]) -> None:
        self._seen.update(acronyms)

    def unseen_acronyms(self, text: str) -> list[str]:
        found = _ACRONYM_RE.findall(text)
        unseen: list[str] = []
        for a in found:
            if a not in self._seen and a not in unseen:
                unseen.append(a)
        return unseen

    def _cooldown_active(self) -> bool:
        if self._cooldown_seconds <= 0:
            return False
        if self._last_trigger_at <= 0:
            return False
        return (time.time() - self._last_trigger_at) < self._cooldown_seconds

    def should_trigger(self, segment: TranscriptSegment) -> bool:
        if segment.speaker != "client":
            return False
        if segment.language != "en":
            return False
        if not self.unseen_acronyms(segment.text):
            return False
        if self._cooldown_active():
            return False
        return True

    def _skeleton_content(self, acronyms: list[str]) -> str:
        if len(acronyms) == 1:
            return f"{acronyms[0]} = …"
        return "、".join(acronyms) + " = …"

    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult:
        results = [r async for r in self.analyze_stream(segment, context)]
        return results[-1] if results else PluginResult(
            plugin_name=self.name,
            ui_section=self.ui_section,
            content="",
            priority=0,
        )

    async def analyze_stream(
        self, segment: TranscriptSegment, context: ConversationContext
    ) -> AsyncGenerator[PluginResult, None]:
        acronyms = self.unseen_acronyms(segment.text)
        if not acronyms:
            return
        # Reserve acronyms before the LLM call to avoid duplicate parallel triggers
        self.mark_seen(acronyms)
        self._last_trigger_at = time.time()
        result_id = str(uuid.uuid4())

        yield PluginResult(
            plugin_name=self.name,
            ui_section=self.ui_section,
            content=self._skeleton_content(acronyms),
            priority=1,
            result_id=result_id,
            status="skeleton",
        )

        prompt = (
            f"客户说：\"{segment.text}\"\n\n"
            f"请解释其中出现的术语/缩写：{', '.join(acronyms)}"
        )
        content = await self._llm.complete(prompt=prompt, system=_SYSTEM_PROMPT)
        yield PluginResult(
            plugin_name=self.name,
            ui_section=self.ui_section,
            content=content,
            priority=1,
            result_id=result_id,
            status="final",
        )
