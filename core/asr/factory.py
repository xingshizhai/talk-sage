from typing import Any
from core.asr.base import ASREngine
from core.asr.dual_engine import DualASREngine
from core.asr.faster_whisper_engine import FasterWhisperEngine
from core.asr.funasr_engine import FunASREngine
from core.asr.openai_cloud_engine import OpenAICloudEngine


def build_asr_engine(transcribe_cfg: dict[str, Any] | None) -> ASREngine:
    """Build a DualASREngine for local or cloud mode from transcribe config."""
    cfg = transcribe_cfg or {}
    mode = (cfg.get("mode") or "local").lower()

    if mode == "cloud":
        return _build_cloud(cfg)
    return _build_local(cfg)


def _build_local(cfg: dict[str, Any]) -> DualASREngine:
    client_cfg = cfg.get("client") or {}
    user_cfg = cfg.get("user") or {}
    client_engine = FasterWhisperEngine(
        model_size=client_cfg.get("model", "small"),
        device=client_cfg.get("device", "cpu"),
        compute_type=client_cfg.get("compute_type", "int8"),
        vad_filter=client_cfg.get("vad_filter", True),
        language="en",
    )
    user_engine = FunASREngine(
        model=user_cfg.get("model", "paraformer-zh"),
        device=user_cfg.get("device", "cpu"),
    )
    return DualASREngine(client_engine=client_engine, user_engine=user_engine)


def _build_cloud(cfg: dict[str, Any]) -> DualASREngine:
    cloud = cfg.get("cloud") or {}
    api_key = cloud.get("api_key") or ""
    base_url = cloud.get("base_url")
    model = cloud.get("model") or "whisper-1"
    client_engine = OpenAICloudEngine(
        api_key=api_key,
        base_url=base_url,
        model=model,
        language="en",
    )
    user_engine = OpenAICloudEngine(
        api_key=api_key,
        base_url=base_url,
        model=model,
        language="zh",
    )
    return DualASREngine(client_engine=client_engine, user_engine=user_engine)
