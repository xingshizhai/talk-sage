import pytest
import numpy as np
from unittest.mock import MagicMock, patch
from core.transcribe_engine import TranscribeEngine
from core.models import TranscriptSegment


@pytest.fixture
def mock_whisper_model():
    mock = MagicMock()
    segment = MagicMock()
    segment.text = " our NPI schedule starts in Q3"
    mock.transcribe.return_value = ([segment], MagicMock(language="en"))
    return mock


def test_transcribe_returns_segment(mock_whisper_model):
    engine = TranscribeEngine(model_size="base")
    engine._model = mock_whisper_model

    audio = np.zeros(16000, dtype=np.float32)
    result = engine.transcribe(audio, speaker="client")

    assert isinstance(result, TranscriptSegment)
    assert result.speaker == "client"
    assert result.language == "en"
    assert "NPI" in result.text


def test_transcribe_strips_leading_whitespace(mock_whisper_model):
    engine = TranscribeEngine(model_size="base")
    engine._model = mock_whisper_model

    audio = np.zeros(16000, dtype=np.float32)
    result = engine.transcribe(audio, speaker="client")
    assert not result.text.startswith(" ")


def test_transcribe_returns_none_for_silence():
    engine = TranscribeEngine(model_size="base")
    mock = MagicMock()
    mock.transcribe.return_value = ([], MagicMock(language="en"))
    engine._model = mock

    audio = np.zeros(16000, dtype=np.float32)
    result = engine.transcribe(audio, speaker="client")
    assert result is None


def test_engine_lazy_loads_model():
    engine = TranscribeEngine(model_size="base")
    assert engine._model is None  # not loaded until first use
