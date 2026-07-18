import numpy as np
import pytest
from unittest.mock import MagicMock, patch
from core.asr.parakeet_engine import ParakeetEngine
from core.models import TranscriptSegment


def test_parakeet_transcribe_returns_segment():
    engine = ParakeetEngine(model_id="nemo-parakeet-tdt-0.6b-v3", device="cpu")
    mock_model = MagicMock()
    mock_model.recognize.return_value = "  our NPI schedule  "
    engine._model = mock_model

    result = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")

    assert isinstance(result, TranscriptSegment)
    assert result.text == "our NPI schedule"
    assert result.language == "en"
    mock_model.recognize.assert_called_once()


def test_parakeet_returns_none_on_empty():
    engine = ParakeetEngine()
    mock_model = MagicMock()
    mock_model.recognize.return_value = "   "
    engine._model = mock_model
    assert engine.transcribe(np.zeros(1600, dtype=np.float32), speaker="client") is None


def test_parakeet_raises_when_package_missing():
    engine = ParakeetEngine()
    import builtins
    real_import = builtins.__import__

    def guarded(name, globals=None, locals=None, fromlist=(), level=0):
        if name == "onnx_asr" or name.startswith("onnx_asr."):
            raise ImportError("missing onnx_asr")
        return real_import(name, globals, locals, fromlist, level)

    with patch("builtins.__import__", side_effect=guarded):
        with pytest.raises(RuntimeError, match="onnx-asr"):
            engine.warmup()


def test_factory_builds_parakeet_client_engine():
    from core.asr.factory import build_asr_engine
    from core.asr.parakeet_engine import ParakeetEngine as PE

    with patch("core.device_probe.detect_compute_device", return_value="cpu"):
        engine = build_asr_engine({
            "mode": "local",
            "client": {
                "engine": "parakeet",
                "model": "nemo-parakeet-tdt-0.6b-v3",
                "device": "cpu",
            },
            "user": {"model": "paraformer-zh", "device": "cpu"},
        })
    assert isinstance(engine._client_engine, PE)
