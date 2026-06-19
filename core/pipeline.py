import asyncio
import numpy as np
from typing import Callable
from core.audio_hub import AudioHub
from core.transcribe_engine import TranscribeEngine
from core.plugin_bus import PluginBus
from core.models import TranscriptSegment, PluginResult


class Pipeline:
    def __init__(
        self,
        hub: AudioHub,
        engine: TranscribeEngine,
        bus: PluginBus,
    ):
        self._hub = hub
        self._engine = engine
        self._bus = bus
        self._loop: asyncio.AbstractEventLoop | None = None
        self.on_segment: Callable[[TranscriptSegment], None] | None = None
        self.on_result: Callable[[PluginResult], None] | None = None
        self._hub.on_segment = self._schedule_audio

    def start(self) -> None:
        self._loop = asyncio.new_event_loop()
        import threading
        t = threading.Thread(target=self._loop.run_forever, daemon=True)
        t.start()
        self._hub.start()

    def stop(self) -> None:
        self._hub.stop()
        if self._loop:
            self._loop.call_soon_threadsafe(self._loop.stop)

    def _schedule_audio(self, audio: np.ndarray, speaker: str) -> None:
        if self._loop:
            asyncio.run_coroutine_threadsafe(
                self._handle_audio(audio, speaker), self._loop
            )

    async def _handle_audio(self, audio: np.ndarray, speaker: str) -> None:
        segment = self._engine.transcribe(audio, speaker=speaker)
        if segment is None:
            return
        if self.on_segment:
            self.on_segment(segment)
        async for result in self._bus.process(segment):
            if self.on_result:
                self.on_result(result)
