import io
import numpy as np
import pytest
from unittest.mock import MagicMock, patch
from core.asr.openai_cloud_engine import OpenAICloudEngine, audio_to_wav_bytes
from core.models import TranscriptSegment


def test_audio_to_wav_bytes_has_riff_header():
    audio = np.zeros(1600, dtype=np.float32)
    data = audio_to_wav_bytes(audio, sample_rate=16000)
    assert data[:4] == b"RIFF"
    assert b"WAVE" in data[:16]


def test_cloud_transcribe_returns_segment():
    engine = OpenAICloudEngine(
        api_key="sk-test",
        model="whisper-1",
        language="en",
    )
    mock_client = MagicMock()
    mock_client.audio.transcriptions.create.return_value = MagicMock(text="  our NPI schedule  ")
    engine._client = mock_client

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")

    assert isinstance(result, TranscriptSegment)
    assert result.text == "our NPI schedule"
    assert result.speaker == "client"
    assert result.language == "en"
    mock_client.audio.transcriptions.create.assert_called_once()


def test_cloud_transcribe_returns_none_on_empty():
    engine = OpenAICloudEngine(api_key="sk-test", language="zh")
    mock_client = MagicMock()
    mock_client.audio.transcriptions.create.return_value = MagicMock(text="   ")
    engine._client = mock_client

    assert engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="user") is None


def test_cloud_warmup_creates_client():
    engine = OpenAICloudEngine(api_key="sk-test", base_url="https://api.openai.com/v1")
    with patch("core.asr.openai_cloud_engine.OpenAI") as MockOpenAI:
        MockOpenAI.return_value = MagicMock()
        engine.warmup()
        MockOpenAI.assert_called_once()
        assert engine._client is not None
