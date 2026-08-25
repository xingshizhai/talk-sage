# Punctuation Restoration & Semantic Segmentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After streaming ASR (paraformer-zh / zipformer-en) produces a final segment, run a lightweight sherpa-onnx CT-Transformer punctuation model to insert commas/periods at semantic boundaries, then split the segment on strong sentence-ending punctuation (。！？) into separate sub-segments so unbroken speech with multiple thoughts is correctly divided.

**Architecture:** Four layers: (1) ASR crate gains `PunctuationRestorer` struct wrapping `sherpa_onnx::OfflinePunctuation` + model download helpers; (2) Config gains `punct_enabled: bool`; (3) Pipeline `finish_speech()` applies punct restorer and emits multiple sub-segments when strong boundaries appear; (4) Server/Tauri + UI expose the model as a downloadable asset with an enable/disable toggle. Feature degrades gracefully: if model not downloaded, segmentation works exactly as before.

**Tech Stack:** Rust (sherpa-onnx 1.13 `OfflinePunctuation`), TypeScript, React

**Model:** `sherpa-onnx-punct-ct-transformer-zh-en-vocab500k-2023-04-12` (~20 MB, CPU <5ms/segment). Only applied to streaming engines (paraformer-zh, zipformer-en); offline engines (Whisper, Qwen3) produce their own punctuation.

---

## File Map

| File | Change |
|------|--------|
| `crates/talksage-asr/src/punct.rs` | New: `PunctuationRestorer` struct |
| `crates/talksage-asr/src/models.rs` | Add punct model download/status helpers |
| `crates/talksage-asr/src/lib.rs` | Export `PunctuationRestorer`; re-export punct model helpers |
| `crates/talksage-config/src/lib.rs` | Add `punct_enabled: bool` to `AsrConfig` |
| `crates/talksage-pipeline/src/lib.rs` | `StreamWorker` holds `Option<PunctuationRestorer>`; `finish_speech` applies and splits |
| `crates/talksage-server/src/lib.rs` | Handle `"punct"` model ID in download/remove/status endpoints |
| `web/src-tauri/src/lib.rs` | Handle `"punct"` model ID in Tauri download/remove/status commands |
| `web/src/lib/api.ts` | Add `punct_enabled` to `AppConfig.asr`; add `"punct"` to model APIs |
| `web/src/sections/SettingsSection.tsx` | Add punct toggle in ASR tab; show punct model in model list |

---

## Task 1: `PunctuationRestorer` + model download helpers

**Files:**
- Create: `crates/talksage-asr/src/punct.rs`
- Modify: `crates/talksage-asr/src/models.rs`
- Modify: `crates/talksage-asr/src/lib.rs`

### Background

Model layout (after download):
```
<models_root>/
  punct-ct-transformer/
    model.onnx        ← only required file
```

`PunctuationRestorer::try_load(models_root)` looks for `punct-ct-transformer/model.onnx`. Returns `None` if missing (feature degrades gracefully).

The model handles Chinese, English, and mixed text. After restoring punctuation, we split on `[。！？!?]` to produce sub-segments. Commas stay in-segment (weak boundaries).

- [ ] **Step 1: Write failing tests in `crates/talksage-asr/src/punct.rs`**

Create the new file with tests first:

```rust
//! Punctuation restoration using sherpa-onnx CT-Transformer model.

use std::path::Path;

/// Punct model directory name within the models root.
pub const PUNCT_MODEL_DIR: &str = "punct-ct-transformer";

/// Returns the ONNX model path for the given models root.
pub fn punct_model_path(models_root: &Path) -> std::path::PathBuf {
    models_root.join(PUNCT_MODEL_DIR).join("model.onnx")
}

/// Returns true if the punct model is installed.
pub fn is_punct_model_available(models_root: &Path) -> bool {
    punct_model_path(models_root).exists()
}

/// Wraps `sherpa_onnx::OfflinePunctuation` for segment-level punct restoration.
pub struct PunctuationRestorer {
    inner: sherpa_onnx::OfflinePunctuation,
}

// SAFETY: OfflinePunctuation is Send+Sync (already declared in sherpa-onnx crate)
unsafe impl Send for PunctuationRestorer {}
unsafe impl Sync for PunctuationRestorer {}

impl PunctuationRestorer {
    /// Load from models_root. Returns None if model not installed.
    pub fn try_load(models_root: &Path) -> Option<Self> {
        let model_path = punct_model_path(models_root);
        if !model_path.exists() {
            return None;
        }
        let mut config = sherpa_onnx::OfflinePunctuationConfig::default();
        config.model.ct_transformer = Some(model_path.to_string_lossy().into_owned());
        config.model.num_threads = 1;
        let inner = sherpa_onnx::OfflinePunctuation::create(&config)?;
        Some(Self { inner })
    }

    /// Add punctuation to raw text. Returns original text on failure.
    pub fn add_punctuation(&self, text: &str) -> String {
        self.inner.add_punctuation(text).unwrap_or_else(|| text.to_string())
    }

    /// Add punctuation then split on strong sentence-ending marks (。！？!?).
    ///
    /// Each returned segment carries its proportional share of `total_duration_ms`.
    /// Sub-segments shorter than `min_chars` are merged into the previous one.
    pub fn restore_and_split(
        &self,
        text: &str,
        total_duration_ms: u64,
        min_chars: usize,
    ) -> Vec<(String, u64)> {
        let punctuated = self.add_punctuation(text);
        split_on_strong_boundaries(&punctuated, total_duration_ms, min_chars)
    }
}

/// Split `text` on 。！？!? boundaries, allocating duration proportionally by char count.
/// Fragments shorter than `min_chars` are merged into the previous segment.
pub fn split_on_strong_boundaries(
    text: &str,
    total_duration_ms: u64,
    min_chars: usize,
) -> Vec<(String, u64)> {
    // Collect split positions: index of each strong-boundary char.
    let chars: Vec<char> = text.chars().collect();
    let total_chars = chars.len();
    if total_chars == 0 {
        return vec![];
    }

    let is_boundary = |c: char| matches!(c, '。' | '！' | '？' | '!' | '?');

    // Build segments: split AFTER each boundary char (include it in the left segment).
    let mut raw: Vec<String> = Vec::new();
    let mut start = 0usize;
    for i in 0..total_chars {
        if is_boundary(chars[i]) {
            let seg: String = chars[start..=i].iter().collect();
            raw.push(seg);
            start = i + 1;
        }
    }
    // Remainder after last boundary.
    if start < total_chars {
        let seg: String = chars[start..].iter().collect();
        raw.push(seg);
    }

    // Merge fragments shorter than min_chars into the previous segment.
    let mut merged: Vec<String> = Vec::new();
    for seg in raw {
        let seg = seg.trim().to_string();
        if seg.is_empty() {
            continue;
        }
        if seg.chars().count() < min_chars {
            if let Some(prev) = merged.last_mut() {
                prev.push_str(&seg);
            } else {
                merged.push(seg);
            }
        } else {
            merged.push(seg);
        }
    }

    if merged.is_empty() {
        return vec![(text.to_string(), total_duration_ms)];
    }

    // Distribute duration proportionally by char count.
    let total_seg_chars: usize = merged.iter().map(|s| s.chars().count()).sum();
    let mut result: Vec<(String, u64)> = Vec::with_capacity(merged.len());
    let mut allocated_ms: u64 = 0;
    let n = merged.len();
    for (i, seg) in merged.into_iter().enumerate() {
        let dur = if i == n - 1 {
            total_duration_ms.saturating_sub(allocated_ms)
        } else {
            let chars_here = seg.chars().count();
            let ms = (total_duration_ms as f64 * chars_here as f64 / total_seg_chars as f64)
                .round() as u64;
            allocated_ms += ms;
            ms
        };
        result.push((seg, dur));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punct_model_path_is_correct() {
        let root = std::path::Path::new("/models");
        assert_eq!(
            punct_model_path(root),
            root.join("punct-ct-transformer").join("model.onnx")
        );
    }

    #[test]
    fn is_punct_model_available_false_when_missing() {
        let tmp = std::env::temp_dir().join("talksage-punct-test-absent");
        assert!(!is_punct_model_available(&tmp));
    }

    #[test]
    fn split_on_strong_boundaries_single_sentence() {
        let segs = split_on_strong_boundaries("你好世界。", 1000, 2);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "你好世界。");
        assert_eq!(segs[0].1, 1000);
    }

    #[test]
    fn split_on_strong_boundaries_two_sentences() {
        // "就是再买一个，我比划比划那种成色。"  — comma stays in segment, period splits.
        // No 。 in the middle → stays as one; let's test a clear two-sentence case.
        let text = "你好世界。我很高兴认识你。";
        let segs = split_on_strong_boundaries(text, 1000, 2);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].0, "你好世界。");
        assert_eq!(segs[1].0, "我很高兴认识你。");
        // Total duration should sum to 1000.
        assert_eq!(segs.iter().map(|s| s.1).sum::<u64>(), 1000);
    }

    #[test]
    fn split_on_strong_boundaries_merges_short_tail() {
        // "A。B。" — "B。" is 2 chars which equals min_chars=2, not less, so NOT merged.
        let segs = split_on_strong_boundaries("AAAA。B。", 1000, 3);
        // "B。" is 2 chars < min_chars 3, gets merged into "AAAA。"
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "AAAA。B。");
    }

    #[test]
    fn split_proportional_duration() {
        // "AAAA。BBBB。" — equal length, should split 50/50.
        let segs = split_on_strong_boundaries("AAAA。BBBB。", 1000, 2);
        assert_eq!(segs.len(), 2);
        // Each segment is 5 chars (including the 。), so 500ms each.
        let total: u64 = segs.iter().map(|s| s.1).sum();
        assert_eq!(total, 1000);
        // Both should be roughly equal (within 1ms of 500).
        assert!((segs[0].1 as i64 - 500).abs() <= 1);
    }

    #[test]
    fn split_empty_text() {
        let segs = split_on_strong_boundaries("", 1000, 2);
        assert!(segs.is_empty());
    }

    #[test]
    fn split_no_boundary() {
        // No strong boundary → single segment.
        let segs = split_on_strong_boundaries("你好，世界", 500, 2);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "你好，世界");
        assert_eq!(segs[0].1, 500);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (or pass for pure logic tests)**

```bash
cargo test -p talksage-asr punct 2>&1 | tail -20
```

The `is_punct_model_available_false_when_missing`, `split_*` tests should pass immediately (no model needed). `PunctuationRestorer` tests need the model and are skipped if not present.

- [ ] **Step 3: Add model download helpers to `crates/talksage-asr/src/models.rs`**

Add these public functions after the existing `remove_engine` function:

```rust
/// Punct model download URL and destination filename.
fn punct_sources() -> Vec<(String, String)> {
    vec![(
        "model.onnx".to_string(),
        "https://huggingface.co/csukuangfj/sherpa-onnx-punct-ct-transformer-zh-en-vocab500k-2023-04-12/resolve/main/model.onnx".to_string(),
    )]
}

/// Approximate download size for the punct model.
pub fn punct_download_size_mb() -> u64 { 20 }

/// True if the punct model ONNX file is present on disk.
pub fn is_punct_model_installed(models_root: &Path) -> bool {
    crate::punct::is_punct_model_available(models_root)
}

/// Download the punctuation model into `<models_root>/punct-ct-transformer/`.
///
/// Progress is reported via `tx` (bytes_done, total_bytes).
/// If `cancel` is set to `true`, the download stops and partial files are cleaned up.
pub fn download_punct_model(
    models_root: &Path,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    tx: Option<std::sync::mpsc::Sender<(u64, u64)>>,
) -> anyhow::Result<()> {
    use crate::punct::PUNCT_MODEL_DIR;
    let dir = models_root.join(PUNCT_MODEL_DIR);
    if is_punct_model_installed(models_root) {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    for (filename, url) in punct_sources() {
        let dest = dir.join(&filename);
        let part = dir.join(format!("{filename}.part"));
        download_file(&url, &part, cancel.clone(), tx.clone())?;
        std::fs::rename(&part, &dest)?;
    }
    Ok(())
}

/// Remove the punct model directory.
pub fn remove_punct_model(models_root: &Path) -> std::io::Result<()> {
    use crate::punct::PUNCT_MODEL_DIR;
    let dir = models_root.join(PUNCT_MODEL_DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Export from `crates/talksage-asr/src/lib.rs`**

Add to `lib.rs`:

```rust
pub mod punct;
pub use punct::{PunctuationRestorer, is_punct_model_available};
pub use models::{download_punct_model, remove_punct_model, is_punct_model_installed, punct_download_size_mb};
```

(Add alongside existing `pub use models::...` lines.)

- [ ] **Step 5: Build and test**

```bash
cargo test -p talksage-asr punct 2>&1 | tail -20
cargo build -p talksage-asr 2>&1 | tail -10
```

Expected: all `split_*` and `is_punct_model_available_false_when_missing` tests pass. `try_load` returns `None` in CI (no model file).

- [ ] **Step 6: Commit**

```bash
git add crates/talksage-asr/src/punct.rs crates/talksage-asr/src/models.rs crates/talksage-asr/src/lib.rs
git commit -m "feat(asr): add PunctuationRestorer and punct model download helpers"
```

---

## Task 2: Config + pipeline integration

**Files:**
- Modify: `crates/talksage-config/src/lib.rs`
- Modify: `crates/talksage-pipeline/src/lib.rs`

### Background

`StreamWorker` is the struct that runs inside the audio thread. It calls `finish_speech()` when VAD detects end-of-speech. We add an `Option<PunctuationRestorer>` field. We set `apply_punct: bool` based on whether the current engine is streaming (paraformer-zh / zipformer-en) — offline engines produce their own punctuation.

The punct restorer is initialized in `TalkSageService::build_live_config_with()` (or wherever `StreamWorker` is constructed) and passed in.

Splitting: `restore_and_split()` returns `Vec<(String, u64)>`. We loop and emit one `DomainEvent::Segment` per sub-segment, with timestamps computed from the proportional duration. The first sub-segment gets `ts_ms = original ts_ms`; each subsequent one gets `ts_ms += prev_duration_ms`.

- [ ] **Step 1: Add `punct_enabled` to `AsrConfig` in `crates/talksage-config/src/lib.rs`**

In `AsrConfig`, add the field after `backend`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    #[serde(alias = "user_engine")]
    pub engine_zh: String,
    #[serde(alias = "client_engine")]
    pub engine_en: String,
    pub backend: String,
    /// 是否启用标点恢复（仅对流式引擎 paraformer-zh / zipformer-en 生效；离线引擎自带标点）。
    /// 需要 punct-ct-transformer 模型已下载。
    pub punct_enabled: bool,
    pub terminology: TerminologyConfig,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            engine_zh: "paraformer-zh".into(),
            engine_en: "zipformer-en".into(),
            backend: "auto".into(),
            punct_enabled: true,
            terminology: TerminologyConfig::default(),
        }
    }
}
```

Update `merge_config` to include `punct_enabled`:

```rust
asr: AsrConfig {
    engine_zh: take_or(user.asr.engine_zh, default.asr.engine_zh),
    engine_en: take_or(user.asr.engine_en, default.asr.engine_en),
    backend: take_or(user.asr.backend, default.asr.backend),
    punct_enabled: user.asr.punct_enabled,
    terminology: user.asr.terminology,
},
```

- [ ] **Step 2: Write a failing test for `punct_enabled` default**

In `crates/talksage-config/src/lib.rs` tests:

```rust
#[test]
fn asr_punct_enabled_defaults_to_true() {
    let cfg = Config::default();
    assert!(cfg.asr.punct_enabled, "标点恢复默认应开启");
}
```

Run: `cargo test -p talksage-config asr_punct_enabled 2>&1 | tail -5`
Expected: FAIL (field doesn't exist yet). Then add the field → PASS.

- [ ] **Step 3: Add `punct_restorer` field to `StreamWorker` in `crates/talksage-pipeline/src/lib.rs`**

Find the `StreamWorker` struct definition. Add the field:

```rust
struct StreamWorker {
    // ... existing fields ...
    /// Punct restorer; None if disabled, model not installed, or offline engine.
    punct_restorer: Option<talksage_asr::PunctuationRestorer>,
    /// True if the current engine is a streaming model (no built-in punctuation).
    apply_punct: bool,
}
```

In `StreamWorker::new()` (or wherever the struct is initialized), add initialization:

```rust
let apply_punct = cfg.asr.punct_enabled && engine_kind.profile().streaming;
let punct_restorer = if apply_punct {
    talksage_asr::PunctuationRestorer::try_load(&models_dir)
} else {
    None
};
```

Where `engine_kind: EngineKind` is already available at construction time (it's used to select the engine), and `models_dir` is the path to the models directory.

- [ ] **Step 4: Apply punct restorer in `finish_speech()`**

In `finish_speech()`, after `terminology.correct()` and before `hooks.apply_filters()`:

Find the block that builds the single `DomainEvent::Segment` for a final segment. Replace it with a loop that emits one event per sub-segment:

**Before (current code pattern):**
```rust
let final_text = engine.finish().trim().to_string();
let final_text = self.terminology.correct(&final_text);

// ... build single ev ...
let ev = DomainEvent::Segment {
    segment: TranscriptSegment {
        text: final_text,
        ts_ms,
        duration_ms,
        // ...
    },
    is_partial: false,
    // ...
};
if let Some(ev) = self.hooks.apply_filters(ev) {
    emit(ev);
    self.on_final(...);
}
```

**After:**
```rust
let raw_text = engine.finish().trim().to_string();
let corrected = self.terminology.correct(&raw_text);

// Punctuation restoration + semantic splitting (streaming engines only).
let sub_segments: Vec<(String, u64)> = if let Some(ref restorer) = self.punct_restorer {
    if corrected.is_empty() {
        vec![]
    } else {
        restorer.restore_and_split(&corrected, duration_ms, 3)
    }
} else {
    if corrected.is_empty() { vec![] } else { vec![(corrected, duration_ms)] }
};

let mut offset_ms: u64 = 0;
for (text, sub_dur) in sub_segments {
    let sub_ts = ts_ms + offset_ms;
    offset_ms += sub_dur;
    let ev = DomainEvent::Segment {
        segment: TranscriptSegment {
            text,
            ts_ms: sub_ts,
            duration_ms: sub_dur,
            speaker_id: segment.speaker_id,
            speaker_label: segment.speaker_label.clone(),
            speaker_attribution: segment.speaker_attribution.clone(),
            is_partial: false,
            rms: segment.rms,
        },
        is_partial: false,
        revision: 0, // will be stamped by SessionRuntime
        start_sample: segment.start_sample,
        end_sample: segment.end_sample,
    };
    if let Some(ev) = self.hooks.apply_filters(ev) {
        emit(ev.clone());
        self.on_final(ev);
    }
}
```

**Important:** Read the actual current code in `finish_speech()` carefully before editing. The field names in `DomainEvent::Segment` and `TranscriptSegment` must match exactly what's in the codebase. Adapt the pattern above to the real struct fields. Do not guess field names — read the structs first.

- [ ] **Step 5: Build pipeline**

```bash
cargo build -p talksage-pipeline 2>&1 | tail -20
```

Fix any compilation errors (field name mismatches, missing imports, etc.). `use talksage_asr::PunctuationRestorer;` should already be in scope if `talksage-asr` is a dependency.

- [ ] **Step 6: Run all workspace tests**

```bash
cargo test --workspace 2>&1 | grep -E "FAILED|test result" | tail -15
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/talksage-config/src/lib.rs crates/talksage-pipeline/src/lib.rs
git commit -m "feat(pipeline): apply punctuation restoration and semantic splitting in finish_speech"
```

---

## Task 3: Server + Tauri API extension for punct model management

**Files:**
- Modify: `crates/talksage-server/src/lib.rs`
- Modify: `web/src-tauri/src/lib.rs`

### Background

The frontend model management UI calls `/api/asr/models/{engine}/download`, `/api/asr/models/{engine}/remove`, and reads `/api/asr/models` for status. We extend these to accept `"punct"` as a special model ID alongside the existing `EngineKind` names.

The existing download handler uses `EngineKind::from_name(engine)`. We add a `"punct"` branch before that check.

- [ ] **Step 1: Extend `/api/asr/models` GET response in server**

In `asr_models_api` handler (in `crates/talksage-server/src/lib.rs`), add a punct entry to the returned models list. Find where the list of engine models is built and append:

```rust
// After building the per-EngineKind entries:
let models_root = state.config.models_dir();
let punct_entry = serde_json::json!({
    "id": "punct",
    "label": "标点恢复模型",
    "description": "CT-Transformer 中英文标点预测，用于流式引擎语义分句",
    "installed": talksage_asr::is_punct_model_installed(&models_root),
    "downloading": talksage_asr::models::is_downloading_punct(&models_root),
    "size_mb": talksage_asr::punct_download_size_mb(),
    "streaming": false,
    "speed": "realtime",
});
// Add punct_entry to the models array before returning.
```

Also add `is_downloading_punct` to `models.rs`:

```rust
pub fn is_downloading_punct(models_root: &Path) -> bool {
    use crate::punct::PUNCT_MODEL_DIR;
    models_root.join(format!("{}.part", PUNCT_MODEL_DIR)).exists()
        || models_root.join(PUNCT_MODEL_DIR).join("model.onnx.part").exists()
}
```

- [ ] **Step 2: Extend `download_model_api` to handle `"punct"` in server**

In `download_model_api`, before the existing `EngineKind::from_name(engine)` lookup, add:

```rust
if engine == "punct" {
    let models_root = state.config.models_dir();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut downloads = state.downloads.lock().unwrap();
        downloads.insert("punct".to_string(), cancel.clone());
    }
    let events = state.events.clone();
    let downloads = state.downloads.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            talksage_asr::download_punct_model(&models_root, cancel, None)
        }).await;
        let _ = events.send(talksage_core::DomainEvent::ModelDownloadComplete {
            engine: "punct".to_string(),
        });
        downloads.lock().unwrap().remove("punct");
    });
    return (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "downloading" }))).into_response();
}
```

Note: `DomainEvent::ModelDownloadComplete` may not exist. If it doesn't, just send a log event or omit the WebSocket notification for now.

- [ ] **Step 3: Extend `remove_model_api` and `cancel_model_download_api` in server**

In `remove_model_api`, before the EngineKind lookup:

```rust
if engine == "punct" {
    talksage_asr::remove_punct_model(&state.config.models_dir())
        .map_err(|e| format!("{e}"))?;
    return (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
}
```

In `cancel_model_download_api`:

```rust
if engine == "punct" {
    let mut downloads = state.downloads.lock().unwrap();
    if let Some(flag) = downloads.get("punct") {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    return (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
}
```

- [ ] **Step 4: Mirror changes in `web/src-tauri/src/lib.rs`**

Find the Tauri command equivalents for model download/remove/status. Apply the same `"punct"` branching pattern.

For model list (the command that returns model info to the Tauri frontend), add the same `punct_entry` JSON object.

For download command, spawn a background thread:
```rust
"punct" => {
    let models_root = config_snapshot.models_dir();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    std::thread::spawn(move || {
        let _ = talksage_asr::download_punct_model(&models_root, cancel, None);
    });
    Ok(serde_json::json!({ "status": "downloading" }))
}
```

For remove command:
```rust
"punct" => {
    talksage_asr::remove_punct_model(&models_root)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true }))
}
```

- [ ] **Step 5: Build both server and Tauri**

```bash
cargo build -p talksage-server 2>&1 | tail -15
cargo build -p talksage-app 2>&1 | tail -15
```

Fix compilation errors. If `DomainEvent::ModelDownloadComplete` doesn't exist, remove that line and just log.

- [ ] **Step 6: Run workspace tests**

```bash
cargo test --workspace 2>&1 | grep -E "FAILED|test result" | tail -10
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/talksage-server/src/lib.rs web/src-tauri/src/lib.rs crates/talksage-asr/src/models.rs
git commit -m "feat(server,tauri): expose punct model in download/remove/status APIs"
```

---

## Task 4: UI — toggle + model download button

**Files:**
- Modify: `web/src/lib/api.ts`
- Modify: `web/src/sections/SettingsSection.tsx`

- [ ] **Step 1: Update `AppConfig.asr` in `web/src/lib/api.ts`**

Add `punct_enabled` to the asr block:

```typescript
asr: {
  engine_zh: string;
  engine_en: string;
  backend: string;
  punct_enabled: boolean;
  terminology: { ... };
};
```

- [ ] **Step 2: Add punct model entry to model list types (if needed)**

If there is an `AsrModel` interface or a model list type in `api.ts`, add `"punct"` as a valid model ID (or widen the type to `string`). If models are typed as `string` already, no change needed.

- [ ] **Step 3: Add `punctEnabled` state in `SettingsSection.tsx`**

In the ASR state section, add:

```tsx
const [punctEnabled, setPunctEnabled] = useState<boolean>(
  config?.asr?.punct_enabled ?? true
);
```

- [ ] **Step 4: Include `punct_enabled` in `handleSave`**

In the `asr` block of the `updates` object:

```tsx
asr: {
  engine_zh: engineZh,
  engine_en: engineEn,
  punct_enabled: punctEnabled,
  terminology: { ... },
},
```

- [ ] **Step 5: Add punct toggle + model status in ASR Tab JSX**

In the ASR tab, after the engine dropdowns section, add:

```tsx
<h3 style={{ ...groupTitle, marginTop: 10 }}>语义分句</h3>
<label style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
  <input
    type="checkbox"
    checked={punctEnabled}
    onChange={(e) => setPunctEnabled(e.target.checked)}
  />
  <span>启用标点恢复（自动在语义边界插入标点并分句）</span>
</label>
<div style={hint}>
  仅对流式引擎（paraformer-zh / zipformer-en）生效；Whisper / Qwen3 自带标点不受影响。
  需要下载「标点恢复模型」（约 20 MB）。关闭后仍按静音断句。
</div>
```

- [ ] **Step 6: Show punct model download button**

After the punct toggle, show the model status and download button. This requires knowing whether the model is installed. Add a state variable and fetch from the models API:

The existing model list API (`/api/asr/models`) now includes the `punct` entry. Read from the already-loaded `models` state (if the component fetches model info) or add a simple `useEffect` fetch:

```tsx
const [punctInstalled, setPunctInstalled] = useState<boolean | null>(null);

useEffect(() => {
  // Fetch model list to check punct status.
  // Use the same fetch pattern as the rest of the settings section.
  fetch("/api/asr/models")
    .then((r) => r.json())
    .then((data: Array<{ id: string; installed: boolean }>) => {
      const entry = data.find((m) => m.id === "punct");
      if (entry) setPunctInstalled(entry.installed);
    })
    .catch(() => {});
}, []);
```

Or, if the settings component already loads model status elsewhere, reuse that. Adapt to match the actual data-loading pattern in the file.

Then render:

```tsx
{punctInstalled === false && punctEnabled && (
  <div style={{ marginTop: 4, display: "flex", alignItems: "center", gap: 8 }}>
    <span style={{ fontSize: 11, color: "var(--warning, #e67e22)" }}>
      标点恢复模型未安装
    </span>
    <button
      type="button"
      onClick={onOpenModels}
      style={{ fontSize: 11, padding: "2px 8px", cursor: "pointer" }}
    >
      打开模型管理下载
    </button>
  </div>
)}
{punctInstalled === true && (
  <div style={{ fontSize: 11, color: "var(--success, #27ae60)", marginTop: 4 }}>
    ✓ 标点恢复模型已安装
  </div>
)}
```

Where `onOpenModels` is the existing prop that opens the model management panel (check the existing ASR tab for the correct prop name).

- [ ] **Step 7: TypeScript compile check**

```bash
cd web && npx tsc --noEmit 2>&1 | head -30
```

Expected: 0 errors. Fix any.

- [ ] **Step 8: Commit**

```bash
git add web/src/lib/api.ts web/src/sections/SettingsSection.tsx
git commit -m "feat(ui): add punct_enabled toggle and model status in ASR tab"
```

---

## Self-Review

### Spec coverage

| Requirement | Covered |
|-------------|---------|
| 中文语义边界分句（例："就是再买一个，我比划比划那种成色"分为两段） | Task 2 Step 4: `restore_and_split` on strong boundaries |
| 轻量模型，低延迟 (<5ms/段) | CT-Transformer, Task 1 |
| 离线模型（Whisper/Qwen3）不受影响 | Task 2 Step 3: `apply_punct` only if `engine_kind.profile().streaming` |
| 模型未下载时功能降级（不崩溃） | Task 1: `try_load` returns `None`; Task 2 Step 4: None branch uses original text |
| 用户可下载模型 | Task 3: server+tauri download API; Task 4: UI download button |
| 用户可开关功能 | Task 2 Step 1: `punct_enabled` config; Task 4: checkbox |
| 持续时长正确分配到子段 | Task 1 Step 1: `split_on_strong_boundaries` proportional duration; tested |

### Placeholder scan

No "TBD", "TODO", or "implement later" in this plan. Task 2 Step 4 says "read the actual current code" — this is a deliberate instruction, not a placeholder.

### Type consistency

- `punct_enabled: bool` in Rust `AsrConfig` ↔ `punct_enabled: boolean` in TypeScript `AppConfig.asr` — consistent.
- `PunctuationRestorer::restore_and_split` returns `Vec<(String, u64)>` — matches the emit loop in Task 2 Step 4.
- `download_punct_model` / `remove_punct_model` / `is_punct_model_installed` exported from `talksage-asr` — used in Task 3 server and tauri code.
