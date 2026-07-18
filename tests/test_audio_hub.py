import pytest
import numpy as np
from unittest.mock import MagicMock, patch
from core.audio_hub import AudioHub


def test_audio_hub_initial_state():
    hub = AudioHub(sample_rate=16000, chunk_seconds=2)
    assert not hub.is_recording


def test_audio_hub_accumulates_audio():
    hub = AudioHub(sample_rate=16000, chunk_seconds=2)
    chunk = np.zeros((1600, 1), dtype=np.float32)
    hub._on_mic_data(chunk, None, None, None)
    hub._on_mic_data(chunk, None, None, None)
    assert hub._mic_buffer.shape[0] == 3200


def test_audio_hub_flushes_when_full():
    flushed = []
    hub = AudioHub(sample_rate=16000, chunk_seconds=1)
    hub.on_segment = lambda audio, speaker: flushed.append((audio, speaker))

    # Feed exactly 1 second of audio (16000 samples)
    chunk = np.zeros((16000, 1), dtype=np.float32)
    hub._on_mic_data(chunk, None, None, None)

    assert len(flushed) == 1
    assert flushed[0][1] == "user"
    assert flushed[0][0].shape[0] == 16000


def test_audio_hub_resets_buffer_after_flush():
    hub = AudioHub(sample_rate=16000, chunk_seconds=1)
    hub.on_segment = lambda audio, speaker: None
    chunk = np.zeros((16000, 1), dtype=np.float32)
    hub._on_mic_data(chunk, None, None, None)
    assert hub._mic_buffer.shape[0] == 0


def test_audio_hub_soft_limits_hot_mic_chunk():
    flushed = []
    hub = AudioHub(sample_rate=16000, chunk_seconds=1, soft_limit_enabled=True, ducking_enabled=False)
    hub.on_segment = lambda audio, speaker: flushed.append(audio)
    chunk = np.ones((16000, 1), dtype=np.float32) * 1.8
    hub._on_mic_data(chunk, None, None, None)
    assert len(flushed) == 1
    assert float(np.max(np.abs(flushed[0]))) <= 1.0
