import numpy as np
from typing import Callable
from core.audio_process import soft_limit, apply_ducking, rms


class AudioHub:
    """Captures mic (user) and optionally system loopback (client).

    on_segment(audio: np.ndarray, speaker: str) fires whenever a full chunk
    is ready.  speaker is "user" for mic data, "client" for loopback data.
    """

    def __init__(
        self,
        sample_rate: int = 16000,
        chunk_seconds: int = 3,
        mic_device: int | None = None,
        soft_limit_enabled: bool = True,
        ducking_enabled: bool = True,
        ducking_threshold: float = 0.04,
        ducking_factor: float = 0.35,
    ):
        self._sample_rate = sample_rate
        self._chunk_size = sample_rate * chunk_seconds
        self._mic_device = mic_device
        self._soft_limit_enabled = soft_limit_enabled
        self._ducking_enabled = ducking_enabled
        self._ducking_threshold = ducking_threshold
        self._ducking_factor = ducking_factor
        self._mic_buffer = np.empty((0,), dtype=np.float32)
        self._loopback_buffer = np.empty((0,), dtype=np.float32)
        self._recent_loopback_rms = 0.0
        self._mic_stream = None
        self._loopback_stream = None
        self.is_recording = False
        self.on_segment: Callable[[np.ndarray, str], None] | None = None

    def start(
        self,
        loopback_device: int | None = None,
        mic_device: int | None = None,
    ) -> None:
        """Start mic capture.  Optionally start loopback for the client stream."""
        import sounddevice as sd

        if mic_device is not None:
            self._mic_device = mic_device

        mic_kwargs: dict = {
            "samplerate": self._sample_rate,
            "channels": 1,
            "dtype": "float32",
            "callback": self._on_mic_data,
        }
        if self._mic_device is not None:
            mic_kwargs["device"] = self._mic_device

        self._mic_stream = sd.InputStream(**mic_kwargs)
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
        self._recent_loopback_rms = 0.0

    def _find_loopback_device(self) -> int | None:
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

    def _process(self, samples: np.ndarray, speaker: str) -> np.ndarray:
        audio = samples.astype(np.float32, copy=False)
        if speaker == "client":
            self._recent_loopback_rms = 0.8 * self._recent_loopback_rms + 0.2 * rms(audio)
        elif speaker == "user" and self._ducking_enabled:
            audio = apply_ducking(
                audio,
                loopback_rms=self._recent_loopback_rms,
                threshold=self._ducking_threshold,
                factor=self._ducking_factor,
            )
        if self._soft_limit_enabled:
            audio = soft_limit(audio)
        return audio

    def _flush(self, buffer: np.ndarray, speaker: str) -> np.ndarray:
        while len(buffer) >= self._chunk_size:
            chunk = buffer[: self._chunk_size].copy()
            buffer = buffer[self._chunk_size :]
            chunk = self._process(chunk, speaker)
            if self.on_segment:
                self.on_segment(chunk, speaker)
        return buffer

    def _on_mic_data(self, indata: np.ndarray, frames, time, status) -> None:
        self._mic_buffer = np.concatenate([self._mic_buffer, indata[:, 0]])
        self._mic_buffer = self._flush(self._mic_buffer, "user")

    def _on_loopback_data(self, indata: np.ndarray, frames, time, status) -> None:
        self._loopback_buffer = np.concatenate([self._loopback_buffer, indata[:, 0]])
        self._loopback_buffer = self._flush(self._loopback_buffer, "client")
