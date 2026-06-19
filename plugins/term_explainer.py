import re
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


class TermExplainerPlugin(AnalyzerPlugin):
    name = "term_explainer"
    display_name = "术语解释"
    ui_section = "terms"

    def __init__(self, llm: LLMProvider):
        self._llm = llm

    def should_trigger(self, segment: TranscriptSegment) -> bool:
        if segment.speaker != "client":
            return False
        if segment.language != "en":
            return False
        return bool(_ACRONYM_RE.search(segment.text))

    async def analyze(self, segment: TranscriptSegment, context: ConversationContext) -> PluginResult:
        acronyms = _ACRONYM_RE.findall(segment.text)
        prompt = (
            f"客户说：\"{segment.text}\"\n\n"
            f"请解释其中出现的术语/缩写：{', '.join(set(acronyms))}"
        )
        content = await self._llm.complete(prompt=prompt, system=_SYSTEM_PROMPT)
        return PluginResult(
            plugin_name=self.name,
            ui_section=self.ui_section,
            content=content,
            priority=1,
        )
