import pytest
from unittest.mock import AsyncMock
from core.notes_generator import NotesGenerator
from core.models import TranscriptSegment, ConversationContext, ConversationState
from llm.base import LLMProvider


class MockLLM(LLMProvider):
    def __init__(self, response: str):
        self._response = response
        self.prompts = []

    async def complete(self, prompt: str, system: str) -> str:
        self.prompts.append(prompt)
        return self._response


@pytest.mark.asyncio
async def test_generate_notes_includes_transcript_in_prompt():
    llm = MockLLM("## 纪要\n- 讨论了 NPI")
    gen = NotesGenerator(llm=llm)
    ctx = ConversationContext()
    ctx.add(TranscriptSegment(speaker="client", text="NPI starts Q3", language="en"))
    ctx.state = ConversationState(topic="NPI", open_questions=["MOQ?"])
    notes = await gen.generate(ctx, terms=["NPI = 新产品导入"])
    assert "NPI" in notes
    assert "NPI starts Q3" in llm.prompts[0]


@pytest.mark.asyncio
async def test_generate_notes_empty_transcript():
    llm = MockLLM("empty")
    gen = NotesGenerator(llm=llm)
    notes = await gen.generate(ConversationContext(), terms=[])
    assert notes  # still returns LLM output or a fallback
