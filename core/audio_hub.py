import numpy as np
from typing import Callable


class AudioHub:
    def __init__(self, sample_rate: int = 16000, chunk_seconds: int = 3):
        self._sample_rate = sample_rate
        self._chunk_size = sample_rate * chunk_seconds
        self._mic_buffer = np.empty((0,), dtype=np.float32)
        self._stream = None
        self.is_recording = False
        # Callback: (audio: np.ndarray, speaker: str) -> None
        self.on_segment: Callable[[np.ndarray, str], None] | None = None

    def start(self) -> None:
        import sounddevice as sd
        self._stream = sd.InputStream(
            samplerate=self._sample_rate,
            channels=1,
            dtype="float32",
            callback=self._on_mic_data,
        )
        self._stream.start()
        self.is_recording = True

    def stop(self) -> None:
        if self._stream:
            self._stream.stop()
            self._stream.close()
            self._stream = None
        self.is_recording = False

    def _on_mic_data(self, indata: np.ndarray, frames, time, status) -> None:
        flat = indata[:, 0]
        self._mic_buffer = np.concatenate([self._mic_buffer, flat])
        if len(self._mic_buffer) >= self._chunk_size:
            segment = self._mic_buffer[: self._chunk_size].copy()
            self._mic_buffer = self._mic_buffer[self._chunk_size :]
            if self.on_segment:
                self.on_segment(segment, "user")
