from abc import ABC, abstractmethod
from typing import AsyncGenerator, AsyncIterator


class LLMProvider(ABC):
    @abstractmethod
    async def complete(self, prompt: str, system: str) -> str:
        """Return a single completion string."""

    async def stream(self, prompt: str, system: str) -> AsyncGenerator[str, None]:
        """Stream completion tokens. Default: yield complete() as one chunk."""
        result = await self.complete(prompt=prompt, system=system)
        yield result
