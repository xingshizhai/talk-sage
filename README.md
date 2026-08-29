<p align="center">
  <strong>拓思者 · TalkSage</strong><br/>
  <em>Your personal AI meeting assistant — transcribe, identify speakers, analyze, and summarize.</em>
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#usage">Usage</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#testing">Testing</a> ·
  <a href="#documentation">Documentation</a> ·
  <a href="README_zh-CN.md">中文版</a>
</p>

> **拓思者 (Tuòsī Zhě)** — "Talk" ≈ 拓 (expand), "Sage" ≈ 思 (think): an AI assistant that expands your thinking by turning every meeting into structured knowledge.

**Platform support**: Windows (full features incl. system-loopback dual-stream capture + Vulkan GPU ASR), macOS / Linux (mic-only single stream; loopback capture is Windows-only, macOS ships the mic-permission TCC declaration).

---

## What it does

拓思者 is **local-first**: capture, recording, GPU ASR, speaker attribution, and media decoding run on the device. If you explicitly select Aliyun fallback or configure a remote OpenAI-compatible LLM, audio or transcript text is sent to that configured provider. The app provides real-time transcription, speaker attribution, term explanations, translation, key-point aggregation, knowledge-base briefs, and meeting minutes while preserving raw recordings for regression testing.

## Features

- **Real-time ASR** — VAD segmentation, responsive partial updates, and smart punctuation/sentence display; **denoise is off by default** so faint or distant speech remains recognizable (enable it in Settings for noisy rooms)
- **Local GPU ASR** — Windows x64: **whisper.cpp + Vulkan** (AMD / Intel / NVIDIA, driver-level loader, no extra runtime needed); macOS Apple Silicon: **whisper.cpp + Metal**. Both use Whisper large-v3-turbo Q5_0 (~547 MiB). When a supported GPU is detected, the pipeline routes through it automatically; falls back to Aliyun cloud or CPU.
- **Scene modes** — six complete runtime presets: **Dictation / Conversation / Bilingual / Meeting / Lecture / Custom**. Conversation is the default and uses low-cost channel attribution; only Meeting enables WeSpeaker voiceprint clustering by default. Bilingual explicitly binds Chinese/English models and translation direction to the two input streams.
- **Speaker attribution** — explicit `off / channel / voiceprint` policy instead of a boolean. Channel attribution labels microphone/system-audio roles without loading a model; voiceprint mode identifies the enrolled owner and clusters other speakers as 「客户1」「客户2」…
- **Live meeting intelligence** — term/acronym explanations, real-time translation (en↔zh), rule-based key-point aggregation (questions / requirements / decisions / actions / technical, with numeric & time heuristics), knowledge-base brief retrieval; History offers **AI-extracted key points** (LLM, needs config)
- **Meeting minutes** — template-based generation via any OpenAI-compatible LLM (DeepSeek, Kimi, Ollama, …)
- **Recording & testing loop** — every session keeps raw mono PCM per input stream and exposes one main recording for playback (single-stream reuse; dual-stream stereo with microphone left/system audio right); `talksage trim` supports regression preparation
- **Live meeting media sessions** — WAV, MP3, and MP4/M4A are locally decoded and then run through the same scene-selected ASR, incremental transcript, pause/stop, term, translation, key-point, recording, and persistence pipeline as microphone sessions. Processing speed is selectable (1×/2×/4×/unlimited), and completion keeps the transcript on screen instead of navigating away.
- **Session quality assessment** — automatic noise/silence detection per session (configurable thresholds + auto background-noise calibration); noisy sessions skip downstream analysis
- **Runtime noise control** — adjust the mic noise gate live from the left panel *while listening*, no restart
- **History** — SQLite sessions with search, per-segment duration/RMS stats, quality badges; **each session auto-saves a runtime snapshot** (scene mode / ASR engines / VAD / denoise / min-commit / gain / speaker mode / app version) so you can compare transcription quality across models & parameters afterwards, or re-run ASR with `talksage session replay <id>` (History detail and `talksage session show <id>`)
- **Two carriers** — Tauri 2 desktop app (IPC) and a headless HTTP/WS server (browser access, token-protected)
- **System tray** — Windows: minimizing hides the window into the tray (click the icon to restore); macOS: follows platform conventions with a menu-bar icon that toggles the window
- **Fixed-corpus benchmark** — `talksage bench` streams through `*.wav` corpora (engine pool warm start, model reused in-process) and reports **CER/WER accuracy + real-time factor (RTF) + first-token latency** for regression testing (borrows WhisperLiveKit's bench)
- **OpenAI-compatible transcription API** — the headless server exposes `POST /v1/audio/transcriptions` and `GET /v1/models`; existing OpenAI-ecosystem clients (whisper-style tools, curl, openai SDK) can point at this machine for **fully local transcription** (Bearer auth, json/text/verbose_json, any-sample-rate wav auto-resampled)
- **Short-segment suppression** — `audio.min_segment_ms` (min commit duration, adjustable in Settings) drops final segments shorter than the threshold, so stray "click/pop" blips in noisy sessions no longer pollute transcripts or history
- **Live conversation metrics** — real-time talk ratio (me vs them), speaking pace (WPM), question count, monologue detection, interruptions, and a **0–100 health score** (pure stats, no LLM; borrows Call.md's conversation-metrics)
- **Live coaching nudges** — rule-driven, 2-minute-cooldown hints (talk imbalance / too few questions / fast pace / confirm next steps near the end) shown as dismissible cards (borrows Call.md's nudge-engine)
- **Trio smart summary** — parallel generation of a narrative overview + **speaker-attributed key points by topic** + **action-item checklist** (History → 智能纪要; borrows Call.md's summary-generator)
- **Meeting-end webhooks** — structured session data (meeting metadata + conversation metrics + quality + notes + full transcript) pushed to n8n/Zapier/CRM when a session ends, with **SSRF protection** (private/loopback URLs rejected; configurable in Settings)
- **Structured Markdown export** — one-click single-file export (overview/metrics → notes → trio summary → transcript) from History; desktop also writes to `<data_dir>/exports/`

## Quick Start

### Prerequisites

- **Rust** (stable) with MSVC toolchain on Windows / clang on macOS / gcc on Linux
- **Node.js 18+** (frontend build)
- **Python 3** (model download script, stdlib only)
- Windows: **VS 2022 Build Tools** (C++ workload) for Tauri & sherpa-onnx linking
- Windows x64 GPU ASR (optional, auto-detected by `talksage.ps1`):
  - **Vulkan SDK** — install from [vulkan.lunarg.com](https://vulkan.lunarg.com/sdk/home#windows); the installer sets `VULKAN_SDK` automatically
  - **LLVM** — install from [llvm.org](https://releases.llvm.org/); run `setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"` once after install

### 1. Get the models

**Option A — in-app (recommended)**: open **模型管理** in the left nav and click "Download" (stop listening first). It performs free-space checks, resumes incomplete downloads, verifies model integrity, and stores release-build models in the writable user data directory rather than inside the app bundle.

See [Model management architecture](docs/model-management.md) for model availability, storage resolution, download state, integrity checks, and logging.

**Option B — command line** (batch / offline):

```bash
# via an HTTP/HTTPS proxy if your network requires one:
# export https_proxy=http://127.0.0.1:10808 http_proxy=http://127.0.0.1:10808
python scripts/download_models.py all            # current product/common models
python scripts/download_models.py qwen3-asr      # CUDA/CPU model
python scripts/download_models.py whisper-metal # Apple Silicon Metal model
python scripts/download_models.py legacy        # legacy test models only
```

This downloads into `models/`:

| Model | Purpose |
|---|---|
| `sherpa-onnx-qwen3-asr-0.6b` | Qwen3-ASR 0.6B offline segment-level (int8, ~878 MB; distributed via official GitHub release, HF repo is gated) |
| `whisper.cpp-large-v3-turbo-q5_0` | GPU ASR: Windows Vulkan (AMD/Intel/NVIDIA) + macOS Metal (~547 MiB; whisper.cpp adapter) |
| `silero-vad/silero_vad.onnx` | Voice activity detection |
| `wespeaker/wespeaker_zh_cnceleb_resnet34.onnx` | Speaker embedding (voiceprint) |

Paraformer, Zipformer, and sherpa ONNX Whisper have been removed from the product model catalog. Existing directories are not deleted automatically because they may contain test fixtures; use the explicit `legacy` script target only for old benchmarks.

The default high-accuracy path is segment-level local GPU ASR: Windows x64 uses Whisper large-v3-turbo Q5_0 through whisper.cpp/Vulkan (AMD/Intel/NVIDIA), Apple Silicon uses the same model through whisper.cpp/Metal, and NVIDIA CUDA uses Qwen3-ASR. Machines without a supported GPU backend fall back to Aliyun realtime ASR (requires API credentials). Legacy streaming Paraformer/Zipformer and explicit local CPU remain available for diagnostics.

### 2. Build

**Windows**

```powershell
.\scripts\talksage.ps1 env      # environment check
.\scripts\talksage.ps1 build    # cargo + frontend (debug: CLI + runnable debug app)
.\scripts\talksage.ps1 build --release  # cargo + frontend (release, no installer)
.\scripts\talksage.ps1 run      # run desktop app (debug); add --release for release
```

`talksage.ps1 dev / build / package` automatically detects and sets the Vulkan build environment (Vulkan SDK, LIBCLANG_PATH, a short `CARGO_TARGET_DIR=C:\wt` to avoid the Windows MAX_PATH limit with `vulkan-shaders-gen`, and static-CRT RUSTFLAGS). If your Vulkan SDK or LLVM are installed in non-default paths, copy [`scripts/talksage.local.example.ps1`](scripts/talksage.local.example.ps1) to `scripts/talksage.local.ps1` (gitignored) and set your paths there — it is sourced automatically before every command.

**macOS / Linux**

```bash
./scripts/talksage.sh env       # environment check
./scripts/talksage.sh build     # cargo + frontend (debug: CLI + runnable debug app)
./scripts/talksage.sh build --release  # cargo + frontend (release, no dmg)
./scripts/talksage.sh run       # run desktop app (debug); add --release for release
./scripts/talksage.sh package   # package dmg / TalkSage.app
```

See [BUILDING.md](docs/BUILDING.md) for full manual steps (static sherpa-onnx linking, proxy notes, packaging).

### 3. Run

```bash
# Desktop app (debug by default; add --release for the release build)
./scripts/talksage.ps1 run              # Windows (debug)
./scripts/talksage.ps1 run --release    # Windows (release)
./scripts/talksage.sh run               # macOS / Linux (debug)
./scripts/talksage.sh run --release     # macOS / Linux (release)

# CLI live transcription (mic)
cargo run -p talksage-cli -- listen --input mic

# CLI from a recorded wav (no GUI; print only)
talksage listen --input meeting.wav
talksage transcribe meeting.wav --save          # transcribe and save as a new session
talksage session replay 8                       # re-transcribe a session's recording into a new session

# Headless web service (browser → http://127.0.0.1:8080)
talksage serve --host 127.0.0.1 --port 8080

# Fixed-corpus benchmark (put *.wav + same-name .txt references in bench-corpus/)
talksage bench --dir bench-corpus --engine paraformer-zh

# OpenAI-compatible transcription via curl (whisper-API style; token via TALKSAGE_SERVER_TOKEN)
curl http://127.0.0.1:8080/v1/audio/transcriptions \
  -H "Authorization: Bearer $TALKSAGE_SERVER_TOKEN" \
  -F file=@meeting.wav -F model=paraformer-zh -F response_format=json
```

## Usage

| Task | How |
|---|---|
| Start listening | Left panel ▶ 开始监听 (jumps to live transcript) |
| Import meeting media | Live transcript → 导入录音文件 (WAV / MP3 / MP4 / M4A) |
| Register your voice | Settings → 声音标识 → 录制我的声音 (6 s) |
| Tune mic level / noise gate | Left panel while listening: 麦克风电平 meter + noise-gate threshold slider (live, no restart) |
| Trim silence from a recording | `talksage trim rec.wav [-o out.wav] [--preset sensitive\|standard\|strict]` |
| Record raw audio only | `talksage record --seconds 60 [--input loopback]` |
| Offline transcription | `talksage transcribe audio.wav` (`--save` to persist); `talksage import audio.wav` is the `--save` alias |
| Doctor / diagnostics | `talksage doctor` |
| Sessions | `talksage session list/show/search/rename/delete/export/notes/trio`; `talksage session <id>` is show |
| Re-transcribe a session | `talksage session replay <id> [--engine qwen3-asr]` (saves a new session) |
| Models | `talksage models list/download/remove/gpu` (`remove` requires `--yes`) |
| Config | `talksage config path`; `config get [dotted.path]`; `config set <path> <value>` (secrets masked) |
| Logs | `talksage logs [--lines 200]` |
| Offline speaker timeline | `talksage diarize audio.wav [--speakers N]` (pyannote segmentation + WeSpeaker clustering) |
| Fixed-corpus benchmark | `talksage bench [--dir corpus] [--engine paraformer-zh\|zipformer-en] [--limit N]` (CER/WER, RTF, first-token latency) |
| Short-segment suppression | Settings → 音频处理 → 最短提交时长 (ms, 0 = off); or `[audio] min_segment_ms` in config |

### The recording → trim → replay loop

Every listening session auto-saves raw wav per stream to `<data_dir>/sessions/<id>/recordings/` (`talksage record` still writes `<data_dir>/recordings/`). Use them as regression material:

```powershell
.\scripts\recording_loop.ps1        # trim all recordings + replay through real ASR
.\scripts\talksage.ps1 loop
```

Details: [docs/RECORDING.md](docs/RECORDING.md)

## Architecture

![拓思者 architecture](docs/architecture.png)

A Rust workspace (single binary, no Python) with one application service and a transport-neutral domain-event bus:

```
Audio input (mic / Windows loopback / wav) → bounded capture queues
        → fair dual-stream scheduler → Preprocessor → VAD → ASR
        → segment lifecycle → speaker attribution → EventFilter chain
        → DomainEvent → Tauri IPC / WebSocket / CLI
        ├── bounded SessionWriter → SQLite + WAV metadata
        └── bounded PluginExecutor → observer results
stop → writer barrier → session finalizers (quality / webhook)
```

`TalkSageService` is the single composition root used by the desktop app, headless server, CLI listen/import, and offline transcription. Each stream owns its VAD, ASR engine, sample clock, endpoint state, statistics, and speaker assignment. SQLite and LLM work stay off the audio path; queues are bounded so slow consumers cannot grow memory without limit.

Meeting mode enables online speaker clustering when the WeSpeaker model is installed. Owner enrollment is optional; it names a matching cluster “我”. Other presets keep voiceprint inference off unless Custom selects it. A sliding voiceprint window can split a long VAD segment after a stable speaker change while keeping ASR partial text responsive.

| Crate | Responsibility |
|---|---|
| `talksage-core` | Domain events, sample clock, transcript state, speaker attribution, metrics |
| `talksage-audio` | Mic/loopback capture, resample, denoise, wav IO, silence trim |
| `talksage-asr` | ASR engine adapters: sherpa-onnx streaming, whisper.cpp GPU (Vulkan / Metal), Aliyun cloud |
| `talksage-pipeline` | Shared service, fair dual-stream scheduling, segment lifecycle, bounded plugin/persistence workers |
| `talksage-plugins` | Registry with filters, segment observers, finalizers, config metadata, and 8 built-ins |
| `talksage-session` | SQLite storage, compatible schema migration, quality evaluation |
| `talksage-notes` | Minutes templates + generator |
| `talksage-server` | axum headless service (REST + WS + SPA) |
| `talksage-cli` | Launcher: listen / transcribe / session / models / config / logs / serve / doctor |
| `web/` | Tauri 2 shell + React/Vite/TS UI |

## Testing

```bash
./scripts/talksage.sh test      # macOS/Linux: Rust + frontend + script tests
cargo test --workspace         # Rust: unit + live model tests (auto-skip if models missing)
cd web && npm test             # frontend: 68 tests (current suite)
```

Windows 使用 `.\scripts\run_tests.ps1` 或 `.\scripts\talksage.ps1 test`。

Real-model integration tests cover Chinese/English ASR, dual-stream fairness and events, recording files, structured speaker attribution, silence trim, server API, and noisy-session quality. Model-dependent tests skip with an explicit message when their model files are absent.

## Documentation

- [architecture-v2.md](docs/architecture-v2.md) — current architecture: shared service, bounded workers, plugins, persistence, and sample clock
- [plugin-development.md](docs/plugin-development.md) — plugin lifecycle, implementation guide, testing checklist, and mechanism assessment
- [BUILDING.md](docs/BUILDING.md) — build & packaging guide
- [vulkan-gpu-build.md](docs/vulkan-gpu-build.md) — Windows Vulkan GPU build: toolchain, CRT linking, troubleshooting
- [cli.md](docs/cli.md) — CLI: sessions, transcription, models, config, logs
- [RECORDING.md](docs/RECORDING.md) — recording / trim / regression loop
- [LOGGING.md](docs/LOGGING.md) — structured logging & debugging
- [testing.md](docs/testing.md) — automated testing strategy
- [real-time-transcription.md](docs/real-time-transcription.md) — live transcript behavior, timing, modes, and improvement roadmap
- [evaluation-user-guide.md](docs/evaluation-user-guide.md) — audio evaluation and model-comparison user guide
- [evaluation-framework.md](docs/evaluation-framework.md) — evaluation architecture and metrics
- [terminology.md](docs/terminology.md) — terminology correction, hot words, and evaluation metrics

## Repository layout

```
crates/            Rust workspace domain crates
web/               Tauri 2 + React frontend
scripts/           build/run/test tooling + model downloaders
  talksage.ps1              Windows all-in-one script (auto-configures Vulkan env)
  talksage.local.example.ps1  per-machine path overrides template (copy → talksage.local.ps1)
  build-vulkan.bat          standalone Vulkan GPU build script
  talksage.sh               macOS/Linux equivalent
vendor/            forked crates (whisper-rs-sys: static-CRT patch for Vulkan)
docs/              design & operation docs
models/            runtime models (gitignored, ~1.2 GB, multi-engine optional)
```

## License

[GNU General Public License v3.0](LICENSE)
