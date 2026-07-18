from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class AudioDeviceInfo:
    index: int
    name: str
    is_loopback_candidate: bool = False


_LOOPBACK_KEYWORDS = ("loopback", "stereo mix", "what u hear", "monitor of")


def _cuda_available() -> bool:
    try:
        import ctranslate2
        return "CUDA" in ctranslate2.get_supported_compute_types("cuda")
    except Exception:
        pass
    try:
        import torch
        return bool(torch.cuda.is_available())
    except Exception:
        return False


def detect_compute_device() -> str:
    """Return 'cuda' if a usable GPU backend is available, else 'cpu'."""
    return "cuda" if _cuda_available() else "cpu"


def recommend_local_asr_settings(device: str | None = None) -> dict[str, str]:
    """Recommend device + compute_type for local faster-whisper / FunASR."""
    resolved = device or detect_compute_device()
    if resolved == "cuda":
        return {"device": "cuda", "compute_type": "float16"}
    return {"device": "cpu", "compute_type": "int8"}


def _query_devices() -> list[dict[str, Any]]:
    import sounddevice as sd
    devices = sd.query_devices()
    return [dict(d) for d in devices]


def list_input_devices() -> list[AudioDeviceInfo]:
    """List sounddevice input devices (mic + loopback candidates)."""
    try:
        raw = _query_devices()
    except Exception:
        return []
    result: list[AudioDeviceInfo] = []
    for i, d in enumerate(raw):
        if int(d.get("max_input_channels") or 0) <= 0:
            continue
        name = str(d.get("name") or f"Device {i}")
        is_lb = any(kw in name.lower() for kw in _LOOPBACK_KEYWORDS)
        result.append(AudioDeviceInfo(index=i, name=name, is_loopback_candidate=is_lb))
    return result


def apply_auto_device_to_config(transcribe_cfg: dict[str, Any]) -> dict[str, Any]:
    """If client/user device is 'auto' or missing compute preference, fill from probe.

    Mutates a shallow-copied config and returns it.
    """
    import copy
    cfg = copy.deepcopy(transcribe_cfg or {})
    if (cfg.get("mode") or "local").lower() != "local":
        return cfg
    rec = recommend_local_asr_settings()
    for key in ("client", "user"):
        section = dict(cfg.get(key) or {})
        device = (section.get("device") or "auto").lower()
        if device in ("auto", ""):
            section["device"] = rec["device"]
            if key == "client":
                ct = (section.get("compute_type") or "auto").lower()
                if ct in ("auto", ""):
                    section["compute_type"] = rec["compute_type"]
        cfg[key] = section
    return cfg
