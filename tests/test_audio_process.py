import numpy as np
from core.audio_process import soft_limit, apply_ducking


def test_soft_limit_caps_peaks():
    audio = np.array([0.5, 1.5, -2.0, 0.1], dtype=np.float32)
    out = soft_limit(audio, threshold=0.95)
    assert float(np.max(np.abs(out))) <= 1.0
    assert out.dtype == np.float32


def test_soft_limit_passthrough_quiet():
    audio = np.array([0.1, -0.2, 0.3], dtype=np.float32)
    out = soft_limit(audio, threshold=0.95)
    np.testing.assert_allclose(out, audio, rtol=1e-5)


def test_ducking_attenuates_mic_when_loopback_loud():
    mic = np.ones(100, dtype=np.float32) * 0.5
    loop = np.ones(100, dtype=np.float32) * 0.8
    ducked = apply_ducking(mic, loopback_rms=float(np.sqrt(np.mean(loop**2))), threshold=0.05, factor=0.35)
    assert float(np.mean(np.abs(ducked))) < float(np.mean(np.abs(mic)))
