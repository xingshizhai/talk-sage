from unittest.mock import patch
from core.asr.factory import build_asr_engine
from core.asr.dual_engine import DualASREngine
from core.asr.faster_whisper_engine import FasterWhisperEngine
from core.asr.funasr_engine import FunASREngine
from core.asr.openai_cloud_engine import OpenAICloudEngine


def test_build_local_dual_engine():
    cfg = {
        "mode": "local",
        "client": {"model": "small", "device": "cpu", "compute_type": "int8", "vad_filter": True},
        "user": {"model": "paraformer-zh", "device": "cpu"},
    }
    engine = build_asr_engine(cfg)
    assert isinstance(engine, DualASREngine)
    assert isinstance(engine._client_engine, FasterWhisperEngine)
    assert isinstance(engine._user_engine, FunASREngine)


def test_build_cloud_dual_engine():
    cfg = {
        "mode": "cloud",
        "cloud": {
            "api_key": "sk-test",
            "base_url": "https://api.openai.com/v1",
            "model": "whisper-1",
        },
    }
    engine = build_asr_engine(cfg)
    assert isinstance(engine, DualASREngine)
    assert isinstance(engine._client_engine, OpenAICloudEngine)
    assert isinstance(engine._user_engine, OpenAICloudEngine)
    assert engine._client_engine._language == "en"
    assert engine._user_engine._language == "zh"


def test_build_defaults_to_local():
    engine = build_asr_engine({})
    assert isinstance(engine, DualASREngine)
    assert isinstance(engine._client_engine, FasterWhisperEngine)


def test_build_local_resolves_auto_device():
    with patch("core.device_probe.detect_compute_device", return_value="cuda"):
        engine = build_asr_engine({
            "mode": "local",
            "client": {"model": "small", "device": "auto", "compute_type": "auto", "vad_filter": True},
            "user": {"model": "paraformer-zh", "device": "auto"},
        })
    assert engine._client_engine._device == "cuda"
    assert engine._client_engine._compute_type == "float16"
    assert engine._user_engine._device == "cuda"
