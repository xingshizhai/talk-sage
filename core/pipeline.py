import asyncio
import threading
import numpy as np
from typing import Callable
from core.audio_hub import AudioHub
from core.asr.base import ASREngine
from core.plugin_bus import PluginBus
from core.models import TranscriptSegment, PluginResult, ConversationState
from core.echo_filter import CrosstalkFilter
from core.session_store import SessionStore


class Pipeline:
    def __init__(
        self,
        hub: AudioHub,
        engine: ASREngine,
        bus: PluginBus,
        echo_filter: CrosstalkFilter | None = None,
        session_store: SessionStore | None = None,
    ):
        self._hub = hub
        self._engine = engine
        self._bus = bus
        self._echo = echo_filter or CrosstalkFilter()
        self._sessions = session_store
        self._loop: asyncio.AbstractEventLoop | None = None
        self.on_segment: Callable[[TranscriptSegment], None] | None = None
        self.on_result: Callable[[PluginResult], None] | None = None
        self.on_asr_status: Callable[[str], None] | None = None
        self.on_state: Callable[[ConversationState], None] | None = None
        self._hub.on_segment = self._schedule_audio

    @property
    def bus(self) -> PluginBus:
        return self._bus

    @property
    def sessions(self) -> SessionStore | None:
        return self._sessions

    @property
    def engine(self) -> ASREngine:
        return self._engine

    def warmup(self) -> None:
        """Pre-load ASR models and report status via on_asr_status."""
        self._emit_asr_status("ASR 加载中…")
        try:
            self._engine.warmup()
        except Exception as exc:
            self._emit_asr_status(f"ASR 失败: {exc}")
            return
        self._emit_asr_status("ASR 就绪")

    def _emit_asr_status(self, message: str) -> None:
        if self.on_asr_status:
            self.on_asr_status(message)

    def start(
        self,
        loopback_device: int | None = None,
        mic_device: int | None = None,
    ) -> None:
        self._loop = asyncio.new_event_loop()
        t = threading.Thread(target=self._loop.run_forever, daemon=True)
        t.start()
        if self._sessions is not None:
            self._sessions.start()
        self._hub.start(loopback_device=loopback_device, mic_device=mic_device)

    def stop(self) -> None:
        self._hub.stop()
        if self._sessions is not None:
            self._sessions.stop()
        if self._loop:
            self._loop.call_soon_threadsafe(self._loop.stop)

    def _schedule_audio(self, audio: np.ndarray, speaker: str) -> None:
        if self._loop:
            asyncio.run_coroutine_threadsafe(
                self._handle_audio(audio, speaker), self._loop
            )

    async def _handle_audio(self, audio: np.ndarray, speaker: str) -> None:
        # Run sync ASR off the event loop so dual streams don't block each other
        loop = asyncio.get_running_loop()
        segment = await loop.run_in_executor(
            None, self._engine.transcribe, audio, speaker
        )
        if segment is None:
            return

        if self._echo.should_drop(segment):
            return
        self._echo.observe(segment)

        if self._sessions is not None:
            self._sessions.add_segment(segment)
        if self.on_segment:
            self.on_segment(segment)
        async for result in self._bus.process(segment):
            if self._sessions is not None:
                self._sessions.add_result(result)
            if self.on_result:
                self.on_result(result)
        if self.on_state:
            self.on_state(self._bus.context.state)
