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
    let chars: Vec<char> = text.chars().collect();
    let total_chars = chars.len();
    if total_chars == 0 {
        return vec![];
    }

    let is_boundary = |c: char| matches!(c, '。' | '！' | '？' | '!' | '?');

    let mut raw: Vec<String> = Vec::new();
    let mut start = 0usize;
    for i in 0..total_chars {
        if is_boundary(chars[i]) {
            let seg: String = chars[start..=i].iter().collect();
            raw.push(seg);
            start = i + 1;
        }
    }
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
        let text = "你好世界。我很高兴认识你。";
        let segs = split_on_strong_boundaries(text, 1000, 2);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].0, "你好世界。");
        assert_eq!(segs[1].0, "我很高兴认识你。");
        assert_eq!(segs.iter().map(|s| s.1).sum::<u64>(), 1000);
    }

    #[test]
    fn split_on_strong_boundaries_merges_short_tail() {
        // "B。" is 2 chars < min_chars 3, gets merged into "AAAA。"
        let segs = split_on_strong_boundaries("AAAA。B。", 1000, 3);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "AAAA。B。");
    }

    #[test]
    fn split_proportional_duration() {
        let segs = split_on_strong_boundaries("AAAA。BBBB。", 1000, 2);
        assert_eq!(segs.len(), 2);
        let total: u64 = segs.iter().map(|s| s.1).sum();
        assert_eq!(total, 1000);
        assert!((segs[0].1 as i64 - 500).abs() <= 1);
    }

    #[test]
    fn split_empty_text() {
        let segs = split_on_strong_boundaries("", 1000, 2);
        assert!(segs.is_empty());
    }

    #[test]
    fn split_no_boundary() {
        let segs = split_on_strong_boundaries("你好，世界", 500, 2);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "你好，世界");
        assert_eq!(segs[0].1, 500);
    }
}
