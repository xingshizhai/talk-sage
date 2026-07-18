from __future__ import annotations

from core.models import ConversationContext
from llm.base import LLMProvider

_SYSTEM = (
    "你是中英双语商务会议秘书。根据转写与上下文，用中文生成简洁会议纪要。"
    "包含：会议主题、关键讨论点、未决问题、已达成共识、跟进事项。"
    "使用 Markdown，条目简短。"
)


class NotesGenerator:
    def __init__(self, llm: LLMProvider):
        self._llm = llm

    async def generate(
        self,
        context: ConversationContext,
        terms: list[str] | None = None,
    ) -> str:
        transcript = context.as_text().strip()
        if not transcript:
            return "（本场会议暂无转写内容，无法生成纪要）"

        state_brief = ""
        if getattr(context, "state", None) is not None:
            state_brief = context.state.as_brief()

        terms_block = "\n".join(f"- {t}" for t in (terms or []) if t) or "- （无）"
        prompt = (
            f"## 对话上下文\n{state_brief}\n\n"
            f"## 术语\n{terms_block}\n\n"
            f"## 转写\n{transcript}\n\n"
            "请生成会议纪要。"
        )
        return await self._llm.complete(prompt=prompt, system=_SYSTEM)
