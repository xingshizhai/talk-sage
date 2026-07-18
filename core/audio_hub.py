import numpy as np
from typing import Callable


class AudioHub:
    """Captures mic (user) and optionally system loopback (client).

    on_segment(audio: np.ndarray, speaker: str) fires whenever a full chunk
    is ready.  speaker is "user" for mic data, "client" for loopback data.
    """

    def __init__(self, sample_rate: int = 16000, chunk_seconds: int = 3):
        self._sample_rate = sample_rate
        self._chunk_size = sample_rate * chunk_seconds
        self._mic_buffer = np.empty((0,), dtype=np.float32)
        self._loopback_buffer = np.empty((0,), dtype=np.float32)
        self._mic_stream = None
        self._loopback_stream = None
        self.is_recording = False
        # Callback: (audio: np.ndarray, speaker: str) -> None
        self.on_segment: Callable[[np.ndarray, str], None] | None = None

    # ── public API ────────────────────────────────────────────────────────────

    def start(self, loopback_device: int | None = None) -> None:
        """Start mic capture.  If loopback_device is given (or auto-detected),
        also start system-audio capture for the client stream."""
        import sounddevice as sd

        self._mic_stream = sd.InputStream(
            samplerate=self._sample_rate,
            channels=1,
            dtype="float32",
            callback=self._on_mic_data,
        )
        self._mic_stream.start()

        device = loopback_device if loopback_device is not None else self._find_loopback_device()
        if device is not None:
            try:
                self._loopback_stream = sd.InputStream(
                    device=device,
                    samplerate=self._sample_rate,
                    channels=1,
                    dtype="float32",
                    callback=self._on_loopback_data,
                )
                self._loopback_stream.start()
            except Exception:
                # Loopback not available on this device; mic-only mode.
                self._loopback_stream = None

        self.is_recording = True

    def stop(self) -> None:
        for stream in [self._mic_stream, self._loopback_stream]:
            if stream:
                stream.stop()
                stream.close()
        self._mic_stream = None
        self._loopback_stream = None
        self.is_recording = False

    # ── internal ──────────────────────────────────────────────────────────────

    def _find_loopback_device(self) -> int | None:
        """Auto-detect WASAPI loopback / Stereo Mix / monitor source."""
        try:
            import sounddevice as sd
            devices = sd.query_devices()
            for i, d in enumerate(devices):
                name = d["name"].lower()
                if any(kw in name for kw in ("loopback", "stereo mix", "what u hear", "monitor of")):
                    return i
        except Exception:
            pass
        return None

    def _flush(self, buffer: np.ndarray, speaker: str) -> np.ndarray:
        """Flush complete chunks from buffer, return leftover."""
        while len(buffer) >= self._chunk_size:
            chunk = buffer[: self._chunk_size].copy()
            buffer = buffer[self._chunk_size :]
            if self.on_segment:
                self.on_segment(chunk, speaker)
        return buffer

    def _on_mic_data(self, indata: np.ndarray, frames, time, status) -> None:
        self._mic_buffer = np.concatenate([self._mic_buffer, indata[:, 0]])
        self._mic_buffer = self._flush(self._mic_buffer, "user")

    def _on_loopback_data(self, indata: np.ndarray, frames, time, status) -> None:
        self._loopback_buffer = np.concatenate([self._loopback_buffer, indata[:, 0]])
        self._loopback_buffer = self._flush(self._loopback_buffer, "client")
