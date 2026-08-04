# BitNet ASR Implementation Plan

> **For agentic workers:** Implement task-by-task; checkboxes track progress.

**Goal:** Wire VibeASR.cpp BitNet CPU ASR into TalkSage for optional live EN ASR and preferred offline import.

**Architecture:** `BitNetEngine` wraps `asr_infer` subprocess; factory + settings + import prefer path.

**Tech Stack:** Python subprocess, existing `ASREngine`, NumPy resample, PySide6 settings.

---

## Files

| File | Action |
|------|--------|
| `core/asr/bitnet_engine.py` | New engine + path resolve + stdout parse |
| `core/asr/factory.py` | `engine: bitnet` + `build_bitnet_engine` / import helper |
| `core/asr/__init__.py` | Export |
| `core/import_audio.py` | `chunk_seconds=0` → whole buffer |
| `main.py` | Import prefer BitNet |
| `ui/settings_dialog.py` | BitNet option |
| `config/defaults.yaml` / `config.template.yaml` | bitnet + import keys |
| `tests/test_bitnet_engine.py` | Mock subprocess |
| docs + README | Document setup |

## Tasks

- [x] Spec written (`2026-08-04-bitnet-asr-design.md`)
- [x] Implement engine + tests
- [x] Factory / config / settings / import
- [x] Docs
