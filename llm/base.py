from abc import ABC, abstractmethod
from typing import AsyncIterator


class LLMProvider(ABC):
    @abstractmethod
    async def complete(self, prompt: str, system: str) -> str:
        """Return a single completion string."""

    async def stream(self, prompt: str, system: str) -> AsyncIterator[str]:
        """Stream completion tokens. Default: yield complete() as one chunk."""
        result = await self.complete(prompt=prompt, system=system)
        yield result
