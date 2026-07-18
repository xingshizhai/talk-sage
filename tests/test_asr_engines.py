import numpy as np
import pytest
from unittest.mock import MagicMock, patch
from core.asr.faster_whisper_engine import FasterWhisperEngine
from core.asr.funasr_engine import FunASREngine
from core.asr.dual_engine import DualASREngine
from core.models import TranscriptSegment


# ── FasterWhisperEngine ───────────────────────────────────────────────────────

@pytest.fixture
def mock_whisper_model():
    seg = MagicMock()
    seg.text = "our NPI schedule starts in Q3"
    model = MagicMock()
    model.transcribe.return_value = ([seg], MagicMock(language="en"))
    return model


def test_faster_whisper_returns_segment(mock_whisper_model):
    engine = FasterWhisperEngine()
    engine._model = mock_whisper_model

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")

    assert isinstance(result, TranscriptSegment)
    assert result.speaker == "client"
    assert result.language == "en"
    assert "NPI" in result.text


def test_faster_whisper_returns_none_on_silence():
    engine = FasterWhisperEngine()
    model = MagicMock()
    model.transcribe.return_value = ([], MagicMock(language="en"))
    engine._model = model

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")
    assert result is None


def test_faster_whisper_strips_whitespace(mock_whisper_model):
    seg = MagicMock()
    seg.text = "  hello world  "
    mock_whisper_model.transcribe.return_value = ([seg], MagicMock(language="en"))
    engine = FasterWhisperEngine()
    engine._model = mock_whisper_model

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")
    assert not result.text.startswith(" ")
    assert not result.text.endswith(" ")


def test_faster_whisper_lazy_load():
    engine = FasterWhisperEngine()
    assert engine._model is None


def test_faster_whisper_passes_language_param(mock_whisper_model):
    engine = FasterWhisperEngine(language="en")
    engine._model = mock_whisper_model

    engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")
    call_kwargs = mock_whisper_model.transcribe.call_args[1]
    assert call_kwargs.get("language") == "en"


def test_faster_whisper_passes_vad_params(mock_whisper_model):
    engine = FasterWhisperEngine(vad_filter=True)
    engine._model = mock_whisper_model

    engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")
    call_kwargs = mock_whisper_model.transcribe.call_args[1]
    assert call_kwargs.get("vad_filter") is True


# ── FunASREngine ──────────────────────────────────────────────────────────────

@pytest.fixture
def mock_funasr_model():
    model = MagicMock()
    model.generate.return_value = [{"text": "这个项目的NPI计划是什么"}]
    return model


def test_funasr_returns_segment(mock_funasr_model):
    engine = FunASREngine()
    engine._model = mock_funasr_model

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="user")

    assert isinstance(result, TranscriptSegment)
    assert result.speaker == "user"
    assert result.language == "zh"
    assert "NPI" in result.text


def test_funasr_returns_none_on_empty(mock_funasr_model):
    mock_funasr_model.generate.return_value = [{"text": ""}]
    engine = FunASREngine()
    engine._model = mock_funasr_model

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="user")
    assert result is None


def test_funasr_returns_none_on_no_result(mock_funasr_model):
    mock_funasr_model.generate.return_value = []
    engine = FunASREngine()
    engine._model = mock_funasr_model

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="user")
    assert result is None


def test_funasr_lazy_load():
    engine = FunASREngine()
    assert engine._model is None


def test_funasr_raises_on_missing_package():
    engine = FunASREngine()
    with patch.dict("sys.modules", {"funasr": None}):
        with pytest.raises(RuntimeError, match="FunASR not installed"):
            engine._load_model()


# ── DualASREngine ─────────────────────────────────────────────────────────────

def test_dual_engine_routes_client_to_whisper():
    client_engine = MagicMock()
    user_engine = MagicMock()
    dual = DualASREngine(client_engine=client_engine, user_engine=user_engine)

    audio = np.zeros(16000, dtype=np.float32)
    dual.transcribe(audio, speaker="client")

    client_engine.transcribe.assert_called_once_with(audio, "client")
    user_engine.transcribe.assert_not_called()


def test_dual_engine_routes_user_to_funasr():
    client_engine = MagicMock()
    user_engine = MagicMock()
    dual = DualASREngine(client_engine=client_engine, user_engine=user_engine)

    audio = np.zeros(16000, dtype=np.float32)
    dual.transcribe(audio, speaker="user")

    user_engine.transcribe.assert_called_once_with(audio, "user")
    client_engine.transcribe.assert_not_called()


def test_dual_engine_warmup_calls_both():
    client_engine = MagicMock()
    user_engine = MagicMock()
    dual = DualASREngine(client_engine=client_engine, user_engine=user_engine)

    dual.warmup()

    client_engine.warmup.assert_called_once()
    user_engine.warmup.assert_called_once()
