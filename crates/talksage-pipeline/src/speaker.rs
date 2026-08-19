//! 说话人识别：声纹注册（主人）+ 在线说话人判定/区分。
//!
//! 用途：多人会议中"先识别出主人（使用方），再区分不同的说话人"。
//! - 设置页注册主人声音 → 保存声纹（embedding）到 `data_dir/voiceprints/owner.vec`
//! - 监听中每个 final 段计算 embedding：与主人比对（匹配 → "我"），
//!   否则与已见说话人比对（匹配 → 复用标签，如"客户1"），都不匹配 → 新建说话人
//! - 模型/声纹缺失时优雅降级：返回流默认标签（保持原双流行为）

use std::path::Path;
use std::sync::{Arc, Mutex};

use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, SpeakerEmbeddingManager};

/// 说话人判定余弦相似度阈值（wespeaker 模型经验值）。
pub const DEFAULT_THRESHOLD: f32 = 0.5;

/// 声纹注册文件名。
pub const OWNER_FILE: &str = "owner.vec";

/// 说话人识别器（多流共享；内部加锁）。
pub struct SpeakerIdentifier {
    extractor: SpeakerEmbeddingExtractor,
    inner: Mutex<Inner>,
    threshold: f32,
}

struct Inner {
    manager: SpeakerEmbeddingManager,
    next_client_id: u32,
}

impl SpeakerIdentifier {
    /// 创建识别器。模型缺失/加载失败 → None（调用方降级）。
    pub fn new(model: &Path, owner_embedding: Option<Vec<f32>>, threshold: f32) -> Option<Self> {
        let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: Some(model.to_string_lossy().into()),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".into()),
        })?;
        let dim = extractor.dim();
        let manager = SpeakerEmbeddingManager::create(dim)?;
        let inner = Inner {
            manager,
            next_client_id: 1,
        };
        let si = Self {
            extractor,
            inner: Mutex::new(inner),
            threshold,
        };
        if let Some(emb) = owner_embedding {
            si.add_owner(&emb);
        }
        Some(si)
    }

    /// 是否已注册主人声纹。
    pub fn has_owner(&self) -> bool {
        self.inner.lock().unwrap().manager.contains("我")
    }

    /// 注册/更新主人声纹（替换旧声纹）。
    pub fn add_owner(&self, embedding: &[f32]) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.manager.remove("我");
        inner.manager.add("我", embedding)
    }

    /// 从一段音频计算声纹 embedding（至少 0.5s，最多 30s 取尾部）。
    pub fn compute_embedding(&self, audio: &[f32]) -> Option<Vec<f32>> {
        const MIN_SAMPLES: usize = 8000; // 0.5s @16k
        const MAX_SAMPLES: usize = 480000; // 30s @16k
        if audio.len() < MIN_SAMPLES {
            return None;
        }
        let start = audio.len().saturating_sub(MAX_SAMPLES);
        let samples = &audio[start..];
        let stream = self.extractor.create_stream()?;
        stream.accept_waveform(16000, samples);
        if !self.extractor.is_ready(&stream) {
            return None;
        }
        self.extractor.compute(&stream)
    }

    /// 判定说话人标签：
    /// 1) 匹配主人 → "我"
    /// 2) 匹配已见说话人 → 复用其标签
    /// 3) 都不匹配 → 新建 "客户N" 并记住
    /// 音频不足或模型失败 → 返回 `fallback`（流默认标签）。
    pub fn identify(&self, audio: &[f32], fallback: &str) -> String {
        let Some(emb) = self.compute_embedding(audio) else {
            return fallback.to_string();
        };
        let mut inner = self.inner.lock().unwrap();
        if let Some(name) = inner.manager.search(&emb, self.threshold) {
            return name;
        }
        let id = inner.next_client_id;
        inner.next_client_id += 1;
        let name = format!("客户{id}");
        inner.manager.add(&name, &emb);
        name
    }

    /// 已知说话人数量（含主人）。
    pub fn num_speakers(&self) -> u32 {
        self.inner.lock().unwrap().manager.num_speakers() as u32
    }
}

/// 保存主人声纹到 `<data_dir>/voiceprints/owner.vec`（JSON：{dim, values}）。
pub fn save_owner_embedding(data_dir: &Path, embedding: &[f32]) -> std::io::Result<()> {
    let dir = data_dir.join("voiceprints");
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::json!({
        "dim": embedding.len(),
        "values": embedding,
    })
    .to_string();
    std::fs::write(dir.join(OWNER_FILE), json)
}

/// 加载主人声纹；不存在/损坏 → None。
pub fn load_owner_embedding(data_dir: &Path) -> Option<Vec<f32>> {
    let raw = std::fs::read_to_string(data_dir.join("voiceprints").join(OWNER_FILE)).ok()?;
    let raw = raw.trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let values = v.get("values")?.as_array()?;
    Some(values.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
}

/// 删除主人声纹。
pub fn remove_owner_embedding(data_dir: &Path) -> std::io::Result<()> {
    let p = data_dir.join("voiceprints").join(OWNER_FILE);
    if p.exists() {
        std::fs::remove_file(p)
    } else {
        Ok(())
    }
}

/// 声纹是否已注册。
pub fn owner_enrolled(data_dir: &Path) -> bool {
    data_dir.join("voiceprints").join(OWNER_FILE).is_file()
}

/// 辅助：Arc 化共享（StreamWorker 用）。
pub type SharedSpeaker = Arc<SpeakerIdentifier>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_embedding_roundtrip() {
        let dir = std::env::temp_dir().join(format!("talksage-spk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let emb: Vec<f32> = (0..128).map(|i| (i as f32) / 128.0).collect();
        save_owner_embedding(&dir, &emb).unwrap();
        assert!(owner_enrolled(&dir));
        let loaded = load_owner_embedding(&dir).unwrap();
        assert_eq!(loaded.len(), 128);
        assert!((loaded[0] - emb[0]).abs() < 1e-6);
        remove_owner_embedding(&dir).unwrap();
        assert!(!owner_enrolled(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_embedding_file_returns_none() {
        let dir = std::env::temp_dir().join(format!("talksage-spk2-{}", std::process::id()));
        assert!(load_owner_embedding(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn identify_falls_back_without_model() {
        // 无模型：SpeakerIdentifier::new 返回 None，identify 不可用 → 由调用方用 fallback
        let model = Path::new("nonexistent-model.onnx");
        assert!(SpeakerIdentifier::new(model, None, DEFAULT_THRESHOLD).is_none());
    }
}
