from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
import numpy as np
from core.asr.base import ASREngine
from core.asr.openai_cloud_engine import audio_to_wav_bytes
from core.models import TranscriptSegment

_VAE_NAME = "vibeasr-vae-encoder-i8_s.gguf"
_LM_NAME = "vibeasr-lm-i2_s-embed-q6_k.gguf"
_BIN_NAMES = ("asr_infer.exe", "asr_infer")


def default_vibeasr_roots() -> list[Path]:
    roots: list[Path] = []
    env = os.environ.get("TALKSAGE_VIBEASR_ROOT", "").strip()
    if env:
        roots.append(Path(env))
    roots.append(Path.home() / ".talksage" / "vibeasr")
    return roots


def resolve_bitnet_paths(
    binary: str | None = None,
    vae_model: str | None = None,
    lm_model: str | None = None,
) -> tuple[Path, Path, Path]:
    """Resolve asr_infer + GGUF paths from config, PATH, or default roots."""
    bin_path = _resolve_binary(binary)
    vae_path = _resolve_file(vae_model, _VAE_NAME, extra_dirs=_model_search_dirs(bin_path))
    lm_path = _resolve_file(lm_model, _LM_NAME, extra_dirs=_model_search_dirs(bin_path))
    return bin_path, vae_path, lm_path


def bitnet_available(
    binary: str | None = None,
    vae_model: str | None = None,
    lm_model: str | None = None,
) -> bool:
    try:
        resolve_bitnet_paths(binary, vae_model, lm_model)
        return True
    except FileNotFoundError:
        return False


def _resolve_binary(configured: str | None) -> Path:
    if configured:
        p = Path(configured).expanduser()
        if p.is_file():
            return p
        raise FileNotFoundError(f"BitNet binary not found: {p}")
    which = shutil.which("asr_infer") or shutil.which("asr_infer.exe")
    if which:
        return Path(which)
    for root in default_vibeasr_roots():
        for name in _BIN_NAMES:
            candidate = root / name
            if candidate.is_file():
                return candidate
            nested = root / "bin" / name
            if nested.is_file():
                return nested
            build = root / "build" / "bin" / name
            if build.is_file():
                return build
    raise FileNotFoundError(
        "BitNet asr_infer not found. Set transcribe.bitnet.binary or "
        "TALKSAGE_VIBEASR_ROOT / ~/.talksage/vibeasr (see README)."
    )


def _model_search_dirs(bin_path: Path) -> list[Path]:
    dirs: list[Path] = []
    for root in default_vibeasr_roots():
        dirs.append(root)
        dirs.append(root / "models")
    # Common layout: .../models/vibeasr next to VibeASR.cpp
    parent = bin_path.parent
    for _ in range(4):
        dirs.append(parent)
        dirs.append(parent / "models")
        dirs.append(parent / "models" / "vibeasr")
        parent = parent.parent
    return dirs


def _resolve_file(configured: str | None, default_name: str, extra_dirs: list[Path]) -> Path:
    if configured:
        p = Path(configured).expanduser()
        if p.is_file():
            return p
        raise FileNotFoundError(f"BitNet model not found: {p}")
    for d in extra_dirs:
        candidate = d / default_name
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(
        f"BitNet model '{default_name}' not found. Set transcribe.bitnet paths or "
        f"place files under ~/.talksage/vibeasr/ (see README)."
    )


def resample_audio(audio: np.ndarray, src_sr: int, dst_sr: int) -> np.ndarray:
    if src_sr == dst_sr or audio.size == 0:
        return np.asarray(audio, dtype=np.float32)
    duration = len(audio) / src_sr
    new_len = max(1, int(duration * dst_sr))
    x_old = np.linspace(0.0, 1.0, num=len(audio), endpoint=False)
    x_new = np.linspace(0.0, 1.0, num=new_len, endpoint=False)
    return np.interp(x_new, x_old, audio).astype(np.float32)


def parse_asr_infer_stdout(stdout: str) -> str:
    """Extract transcript text from asr_infer stdout (logs go to stderr)."""
    text = (stdout or "").strip()
    if not text:
        return ""
    # Prefer last non-empty block; strip JSON-ish speaker lines to plain text if needed
    lines = [ln.strip() for ln in text.splitlines() if ln.strip()]
    if not lines:
        return ""
    # Join multi-line transcripts; drop timing leftovers if mixed
    kept: list[str] = []
    for ln in lines:
        if re.match(r"^=+$", ln):
            continue
        if ln.startswith("Timing") or ln.startswith("Audio:") or ln.startswith("VAE:"):
            continue
        if re.match(r"^RTF:", ln):
            continue
        kept.append(ln)
    if not kept:
        return ""
    # Speaker-formatted lines: [0.00 - 1.20] Speaker 0: hello
    plain: list[str] = []
    for ln in kept:
        m = re.match(r"^\[.*?\]\s*(?:Speaker\s+\S+:\s*)?(.*)$", ln)
        if m and m.group(1):
            plain.append(m.group(1).strip())
        else:
            plain.append(ln)
    return " ".join(plain).strip()


class BitNetEngine(ASREngine):
    """English ASR via VibeASR.cpp BitNet (CPU subprocess).

    Requires a built ``asr_infer`` binary and BitNet GGUF weights.
    See docs/superpowers/specs/2026-08-04-bitnet-asr-design.md.
    """

    def __init__(
        self,
        binary: str | None = None,
        vae_model: str | None = None,
        lm_model: str | None = None,
        threads: int = 4,
        timeout_seconds: float = 600,
        input_sample_rate: int = 16000,
        target_sample_rate: int = 24000,
        language: str = "en",
    ):
        self._binary = binary
        self._vae_model = vae_model
        self._lm_model = lm_model
        self._threads = max(1, int(threads))
        self._timeout = float(timeout_seconds)
        self._input_sr = int(input_sample_rate)
        self._target_sr = int(target_sample_rate)
        self._language = language
        self._resolved: tuple[Path, Path, Path] | None = None

    def warmup(self) -> None:
        self._ensure_resolved()

    def _ensure_resolved(self) -> tuple[Path, Path, Path]:
        if self._resolved is None:
            self._resolved = resolve_bitnet_paths(
                self._binary, self._vae_model, self._lm_model
            )
        return self._resolved

    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None:
        bin_path, vae_path, lm_path = self._ensure_resolved()
        pcm = np.asarray(audio, dtype=np.float32).reshape(-1)
        if pcm.size == 0:
            return None
        pcm24 = resample_audio(pcm, self._input_sr, self._target_sr)
        wav = audio_to_wav_bytes(pcm24, sample_rate=self._target_sr)
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            tmp.write(wav)
            tmp_path = Path(tmp.name)
        try:
            text = self._run_infer(bin_path, vae_path, lm_path, tmp_path)
        finally:
            try:
                tmp_path.unlink(missing_ok=True)
            except OSError:
                pass
        text = (text or "").strip()
        if not text:
            return None
        return TranscriptSegment(speaker=speaker, text=text, language=self._language)

    def _run_infer(
        self,
        bin_path: Path,
        vae_path: Path,
        lm_path: Path,
        audio_path: Path,
    ) -> str:
        cmd = [
            str(bin_path),
            "--vae-model",
            str(vae_path),
            "--lm-model",
            str(lm_path),
            "--audio",
            str(audio_path),
            "-t",
            str(self._threads),
            "--greedy",
            "--prompt-format",
            "text",
            "--sample-rate",
            str(self._target_sr),
        ]
        try:
            proc = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self._timeout,
                check=False,
            )
        except subprocess.TimeoutExpired as e:
            raise RuntimeError(
                f"BitNet asr_infer timed out after {self._timeout}s"
            ) from e
        except OSError as e:
            raise RuntimeError(f"Failed to start BitNet asr_infer: {e}") from e
        if proc.returncode != 0:
            err = (proc.stderr or proc.stdout or "").strip()
            raise RuntimeError(
                f"BitNet asr_infer failed (exit {proc.returncode}): {err[-500:]}"
            )
        return parse_asr_infer_stdout(proc.stdout or "")
