//! 说话人识别：声纹注册（主人）+ 在线说话人判定/区分。
//!
//! 用途：多人会议中"先识别出主人（使用方），再区分不同的说话人"。
//! - 设置页注册主人声音 → 保存声纹（embedding）到 `data_dir/voiceprints/owner.vec`
//! - 监听中每个 final 段计算 embedding：与主人比对（匹配 → "我"），
//!   否则与已见说话人比对（匹配 → 复用标签，如"客户1"），都不匹配 → 新建说话人
//! - 模型/声纹缺失时优雅降级：返回流默认标签（保持原双流行为）

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, SpeakerEmbeddingManager};

/// 说话人判定余弦相似度阈值（wespeaker 模型经验值）。
pub const DEFAULT_THRESHOLD: f32 = 0.5;

/// 声纹注册文件名。
pub const OWNER_FILE: &str = "owner.vec";

const SAMPLE_RATE: usize = 16_000;
const FRAME_SAMPLES: usize = 320; // 20ms
const MIN_VOICED_SAMPLES: usize = SAMPLE_RATE; // 至少 1s 有效人声
const MAX_CLIENT_SPEAKERS: u32 = 8;

/// 裁掉声纹音频两端静音，并拒绝有效人声不足、过弱或静音占比过高的片段。
/// 阈值相对当前片段峰值自适应，同时保留很低的绝对下限。
pub fn prepare_speaker_audio(audio: &[f32]) -> Option<Vec<f32>> {
    if audio.len() < MIN_VOICED_SAMPLES {
        return None;
    }
    let frames = audio
        .chunks(FRAME_SAMPLES)
        .map(|frame| {
            if frame.is_empty() {
                0.0
            } else {
                (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt()
            }
        })
        .collect::<Vec<_>>();
    let peak = frames.iter().copied().fold(0.0f32, f32::max);
    if peak < 0.0005 {
        return None;
    }
    let threshold = (peak * 0.08).max(0.0005);
    let voiced = frames
        .iter()
        .enumerate()
        .filter_map(|(i, rms)| (*rms >= threshold).then_some(i))
        .collect::<Vec<_>>();
    if voiced.len() * FRAME_SAMPLES < MIN_VOICED_SAMPLES {
        return None;
    }
    let first = voiced[0] * FRAME_SAMPLES;
    let end = ((voiced[voiced.len() - 1] + 1) * FRAME_SAMPLES).min(audio.len());
    let cropped = &audio[first..end];
    if voiced.len() * 100 < cropped.len().div_ceil(FRAME_SAMPLES) * 35 {
        return None;
    }
    Some(cropped.to_vec())
}

fn normalized_average(embeddings: &[Vec<f32>]) -> Option<Vec<f32>> {
    let dim = embeddings.first()?.len();
    if dim == 0 || embeddings.iter().any(|e| e.len() != dim) {
        return None;
    }
    let mut avg = vec![0.0f32; dim];
    for emb in embeddings {
        let norm = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= f32::EPSILON {
            return None;
        }
        for (out, value) in avg.iter_mut().zip(emb) {
            *out += *value / norm;
        }
    }
    let norm = avg.iter().map(|x| x * x).sum::<f32>().sqrt();
    (norm > f32::EPSILON).then(|| avg.into_iter().map(|x| x / norm).collect())
}

/// 说话人识别器（多流共享；内部加锁）。
pub struct SpeakerIdentifier {
    /// ONNX extractor 不保证并发调用安全；段内变化检测在后台线程运行，final
    /// 归属可能同时查询，因此统一串行访问底层 extractor。
    extractor: Mutex<SpeakerEmbeddingExtractor>,
    inner: Mutex<Inner>,
    threshold: f32,
}

struct Inner {
    manager: SpeakerEmbeddingManager,
    /// 归一化聚类中心及累计片段数；manager 只作为 sherpa 兼容索引。
    prototypes: HashMap<String, (Vec<f32>, u32)>,
    /// 尚未被第二个相似片段确认的新说话人。必须允许多个候选并存，否则
    /// A/B 交替发言会互相覆盖唯一候选，两个身份都永远无法确认。
    candidates: HashMap<u32, Vec<f32>>,
    next_client_id: u32,
}

enum PendingSpeaker {
    StartCandidate { id: u32, embedding: Vec<f32> },
    ConfirmCandidate { id: u32, embedding: Vec<f32> },
    Update { label: String, embedding: Vec<f32> },
}

/// 一次说话人查询的结果。持有标签；若是新说话人，还持有待注册的声纹。
///
/// 只有把它交给 [`SpeakerIdentifier::commit`] 才会真正注册；丢弃它 = 这次判定
/// 不留痕迹（被 filter 吞掉的段走的就是这条路）。
pub struct SpeakerQuery {
    label: String,
    decision: SpeakerDecision,
    similarity: Option<f32>,
    /// 新说话人：(预分配编号, 待注册 embedding)。已知说话人/降级标签为 None。
    pending: Option<PendingSpeaker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerDecision {
    LowQualityFallback,
    OwnerMatch,
    ExistingMatch,
    GrayZoneReuse,
    CandidateStarted,
    CandidateConfirmed,
    SpeakerLimitFallback,
}

impl SpeakerQuery {
    /// 判定出的标签（"我" / "客户N" / 流默认标签）。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 是否是尚未注册的新说话人。
    pub fn is_new(&self) -> bool {
        matches!(
            self.pending,
            Some(PendingSpeaker::StartCandidate { .. } | PendingSpeaker::ConfirmCandidate { .. })
        )
    }

    pub fn decision(&self) -> SpeakerDecision {
        self.decision
    }

    pub fn similarity(&self) -> Option<f32> {
        self.similarity
    }
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
            prototypes: HashMap::new(),
            candidates: HashMap::new(),
            next_client_id: 1,
        };
        let si = Self {
            extractor: Mutex::new(extractor),
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
        let Some(normalized) = normalize_embedding(embedding) else {
            return false;
        };
        let mut inner = self.inner.lock().unwrap();
        inner.manager.remove("我");
        if !inner.manager.add("我", &normalized) {
            return false;
        }
        inner.prototypes.insert("我".into(), (normalized, 1));
        true
    }

    /// 从一段音频计算声纹 embedding（至少 0.5s，最多 30s 取尾部）。
    pub fn compute_embedding(&self, audio: &[f32]) -> Option<Vec<f32>> {
        const MAX_SAMPLES: usize = 480000; // 30s @16k
        let prepared = prepare_speaker_audio(audio)?;
        let start = prepared.len().saturating_sub(MAX_SAMPLES);
        let samples = &prepared[start..];
        let extractor = self.extractor.lock().ok()?;
        let stream = extractor.create_stream()?;
        stream.accept_waveform(16000, samples);
        if !extractor.is_ready(&stream) {
            return None;
        }
        extractor.compute(&stream)
    }

    /// 注册使用多个独立窗口提取声纹再聚合，避免一次咳嗽、静音或某个词主导模板。
    pub fn enrollment_embedding(&self, audio: &[f32]) -> Option<Vec<f32>> {
        self.enrollment_profile(audio).map(|(embedding, _, _)| embedding)
    }

    /// 返回聚合声纹、裁剪后的有效区间采样数、成功窗口数，供注册 UI 展示质量反馈。
    pub fn enrollment_profile(&self, audio: &[f32]) -> Option<(Vec<f32>, usize, usize)> {
        let prepared = prepare_speaker_audio(audio)?;
        let voiced_samples = prepared.len();
        let window = (prepared.len() / 3).max(MIN_VOICED_SAMPLES);
        let embeddings = prepared
            .chunks(window)
            .take(3)
            .filter_map(|chunk| self.compute_embedding(chunk))
            .collect::<Vec<_>>();
        if embeddings.len() < 2 {
            return None;
        }
        let windows = embeddings.len();
        normalized_average(&embeddings).map(|embedding| (embedding, voiced_samples, windows))
    }

    /// 判定说话人标签（**纯查询**，不改变识别器状态）：
    /// 1) 匹配主人 → "我"
    /// 2) 匹配已见说话人 → 复用其标签
    /// 3) 都不匹配 → 预分配 "客户N"，并把 embedding 一起带回，等调用方 `commit`
    /// 音频不足或模型失败 → 返回 `fallback`（流默认标签）。
    ///
    /// 拆成「查询 + commit」是因为产生点的 filter 链可能把这一段吞掉：
    /// 若查询自带注册副作用，被丢弃的段会留下一个永久的幻影说话人，
    /// 之后真实的段可能匹配上它。所以只有 filter 放行的段才 `commit`。
    pub fn query(&self, audio: &[f32], fallback: &str) -> SpeakerQuery {
        self.query_for_role(audio, fallback, true)
    }

    /// 按业务角色查询。回环/客户流设置 `recognize_owner=false`，即使扬声器回声
    /// 与主人声纹相似，也保持客户通道角色，不把它改写成“我”。
    pub fn query_for_role(&self, audio: &[f32], fallback: &str, recognize_owner: bool) -> SpeakerQuery {
        let Some(emb) = self.compute_embedding(audio) else {
            return SpeakerQuery {
                label: fallback.to_string(),
                decision: SpeakerDecision::LowQualityFallback,
                similarity: None,
                pending: None,
            };
        };
        let Some(emb) = normalize_embedding(&emb) else {
            return SpeakerQuery {
                label: fallback.to_string(),
                decision: SpeakerDecision::LowQualityFallback,
                similarity: None,
                pending: None,
            };
        };
        let inner = self.inner.lock().unwrap();

        // 主人使用更严格阈值，宁可暂时回退通道标签，也不把客户误标为“我”。
        if recognize_owner {
            if let Some((owner, _)) = inner.prototypes.get("我") {
                if cosine_similarity(owner, &emb) >= (self.threshold + 0.05).min(0.95) {
                    return SpeakerQuery {
                        label: "我".into(),
                        decision: SpeakerDecision::OwnerMatch,
                        similarity: Some(cosine_similarity(owner, &emb)),
                        pending: None,
                    };
                }
            }
        }

        let nearest = inner
            .prototypes
            .iter()
            .filter(|(name, _)| name.as_str() != "我")
            .map(|(name, (center, _))| (name, cosine_similarity(center, &emb)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((name, similarity)) = nearest.as_ref() {
            // 灰区仍复用最近客户，避免同一人在音色波动时刷出客户2/3；中心在
            // filter 放行后才更新，低于灰区才认为是明确的新说话人。
            if *similarity >= (self.threshold - 0.08).max(0.0) {
                return SpeakerQuery {
                    label: (*name).clone(),
                    decision: if *similarity >= self.threshold {
                        SpeakerDecision::ExistingMatch
                    } else {
                        SpeakerDecision::GrayZoneReuse
                    },
                    similarity: Some(*similarity),
                    pending: Some(PendingSpeaker::Update { label: (*name).clone(), embedding: emb }),
                };
            }
        }
        let candidate_threshold = (self.threshold - 0.08).max(0.0);
        let candidate = inner.candidates.iter()
            .map(|(id, center)| (*id, cosine_similarity(center, &emb)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, similarity)) = candidate {
            if similarity >= candidate_threshold {
                return SpeakerQuery {
                    label: format!("客户{id}"),
                    decision: SpeakerDecision::CandidateConfirmed,
                    similarity: Some(similarity),
                    pending: Some(PendingSpeaker::ConfirmCandidate { id, embedding: emb }),
                };
            }
        }
        if inner.next_client_id > MAX_CLIENT_SPEAKERS {
            // 防止异常环境持续制造无界聚类；保留通用角色，不污染已有中心。
            return SpeakerQuery {
                label: fallback.to_string(),
                decision: SpeakerDecision::SpeakerLimitFallback,
                similarity: nearest.as_ref().map(|(_, similarity)| *similarity),
                pending: None,
            };
        }
        let id = inner.next_client_id;
        SpeakerQuery {
            // 第一次只显示稳定业务角色；第二个相似片段确认后才显示编号。
            label: fallback.to_string(),
            decision: SpeakerDecision::CandidateStarted,
            similarity: nearest.as_ref().map(|(_, similarity)| *similarity),
            pending: Some(PendingSpeaker::StartCandidate { id, embedding: emb }),
        }
    }

    /// 落库查询结果：仅「新说话人」才真正注册（分配编号 + 写入声纹库）。
    /// 返回 true 表示本次确实注册了新说话人。
    pub fn commit(&self, q: &SpeakerQuery) -> bool {
        let Some(pending) = &q.pending else {
            return false; // 已知说话人或降级标签：无副作用
        };
        let mut inner = self.inner.lock().unwrap();
        match pending {
            PendingSpeaker::StartCandidate { id, embedding } => {
                inner.candidates.insert(*id, embedding.clone());
                inner.next_client_id = inner.next_client_id.max(id + 1);
                false
            }
            PendingSpeaker::ConfirmCandidate { id, embedding } => {
                let label = format!("客户{id}");
                if inner.prototypes.contains_key(&label) || !inner.manager.add(&label, embedding) {
                    return false;
                }
                inner.prototypes.insert(label, (embedding.clone(), 1));
                inner.candidates.remove(id);
                inner.next_client_id = inner.next_client_id.max(id + 1);
                true
            }
            PendingSpeaker::Update { label, embedding } => {
                let Some((center, count)) = inner.prototypes.get_mut(label) else {
                    return false;
                };
                // 最近最多 8 段的平滑权重，既能适应设备/距离变化，又不会被单段带跑。
                let weight = 1.0 / ((*count + 1).min(8) as f32);
                for (value, observed) in center.iter_mut().zip(embedding) {
                    *value = *value * (1.0 - weight) + *observed * weight;
                }
                let updated = normalize_embedding(center).unwrap_or_else(|| center.clone());
                *center = updated.clone();
                *count = count.saturating_add(1);
                inner.manager.remove(label);
                let _ = inner.manager.add(label, &updated);
                false
            }
        }
    }

    /// 查询 + 立即注册。给「拿到标签就一定要用」的调用方（测试、离线工具）用。
    ///
    /// **产生点不要用它**：那里必须先 `query` 拿标签，等 filter 链放行后再
    /// `commit`，否则被吞掉的段会注册幻影说话人。
    pub fn identify(&self, audio: &[f32], fallback: &str) -> String {
        let q = self.query(audio, fallback);
        self.commit(&q);
        q.label
    }

    /// 已知说话人数量（含主人）。
    pub fn num_speakers(&self) -> u32 {
        self.inner.lock().unwrap().prototypes.len() as u32
    }
}

fn normalize_embedding(embedding: &[f32]) -> Option<Vec<f32>> {
    let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON {
        return None;
    }
    Some(embedding.iter().map(|x| x / norm).collect())
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return -1.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
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

    #[test]
    fn speaker_audio_quality_gate_rejects_silence_and_short_clips() {
        assert!(prepare_speaker_audio(&vec![0.0; SAMPLE_RATE * 3]).is_none());
        assert!(prepare_speaker_audio(&vec![0.02; SAMPLE_RATE / 2]).is_none());
    }

    #[test]
    fn speaker_audio_quality_gate_trims_outer_silence() {
        let mut audio = vec![0.0; SAMPLE_RATE];
        audio.extend((0..SAMPLE_RATE * 2).map(|i| ((i as f32 * 0.07).sin()) * 0.05));
        audio.extend(vec![0.0; SAMPLE_RATE]);
        let prepared = prepare_speaker_audio(&audio).expect("两秒有效语音应通过质量门");
        assert!(prepared.len() >= SAMPLE_RATE * 19 / 10);
        assert!(prepared.len() < audio.len());
    }

    #[test]
    fn enrollment_average_is_normalized() {
        let avg = normalized_average(&[vec![3.0, 0.0], vec![0.0, 4.0]]).unwrap();
        let norm = avg.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
