from openai import AsyncOpenAI
from llm.base import LLMProvider


class OpenAICompatProvider(LLMProvider):
    def __init__(self, api_key: str, model: str, base_url: str | None = None):
        self._model = model
        self._client = AsyncOpenAI(api_key=api_key, base_url=base_url)

    async def complete(self, prompt: str, system: str) -> str:
        response = await self._client.chat.completions.create(
            model=self._model,
            messages=[
                {"role": "system", "content": system},
                {"role": "user", "content": prompt},
            ],
        )
        return response.choices[0].message.content or ""
