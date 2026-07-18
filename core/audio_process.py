from __future__ import annotations

import numpy as np


def soft_limit(audio: np.ndarray, threshold: float = 0.95) -> np.ndarray:
    """Scale peaks above *threshold* down to avoid hard clipping distortion."""
    if audio.size == 0:
        return np.asarray(audio, dtype=np.float32)
    x = np.asarray(audio, dtype=np.float32)
    peak = float(np.max(np.abs(x)))
    if peak <= threshold:
        return x
    return np.clip(x * (threshold / peak), -1.0, 1.0).astype(np.float32)


def rms(audio: np.ndarray) -> float:
    if audio.size == 0:
        return 0.0
    return float(np.sqrt(np.mean(np.square(audio, dtype=np.float64))))


def apply_ducking(
    mic: np.ndarray,
    loopback_rms: float,
    threshold: float = 0.04,
    factor: float = 0.35,
) -> np.ndarray:
    """Attenuate mic when loopback is loud (system audio bleeding into mic)."""
    x = np.asarray(mic, dtype=np.float32)
    if loopback_rms < threshold:
        return x
    return (x * factor).astype(np.float32)
