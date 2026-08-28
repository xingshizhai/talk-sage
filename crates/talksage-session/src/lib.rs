//! TalkSage v2 会话持久化：SQLite（rusqlite bundled）。
//!
//! 表：sessions（含 meta JSON）/ segments（含 duration_ms/rms）/ terms / translations / key_points。
//! SessionStore 线程安全（内部 Mutex<Connection>），可由 pipeline 事件线程写入。
//!
//! `sessions.meta` 保存会话级统计与质量评估（SessionMeta），
//! 使"给定时间点 → 完整回溯会话质量/语音占比/能量/录音路径"成为可能。

use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use talksage_core::{KeyPointRecord, TranscriptSegment};

/// 由会话详情构建会议结束 webhook payload（借鉴 Call.md workflow-webhook）。
pub fn build_webhook_payload(detail: &SessionDetail) -> serde_json::Value {
    let duration_secs = detail
        .ended_at
        .map(|e| (e - detail.started_at).max(0))
        .unwrap_or(0);
    let metrics = talksage_core::compute_conversation_metrics(&detail.segments);
    let quality = detail.meta.as_ref().map(|m| {
        serde_json::json!({
            "quality": m.quality,
            "quality_label": m.quality_label(),
            "speech_ratio": m.speech_ratio,
            "text_noise": m.text_noise,
            "skipped_analysis": m.skipped_analysis,
        })
    });
    serde_json::json!({
        "meeting": {
            "id": detail.id,
            "title": detail.title,
            "started_at": detail.started_at,
            "ended_at": detail.ended_at,
            "duration_seconds": duration_secs,
        },
        "metrics": {
            "talk_ratio_me": metrics.talk_ratio_me,
            "talk_ratio_them": metrics.talk_ratio_them,
            "pace_wpm": metrics.pace_wpm,
            "questions_me": metrics.questions_me,
            "monologue_detected": metrics.monologue_detected,
            "interruption_count": metrics.interruption_count,
            "health_score": metrics.health_score,
        },
        "quality": quality,
        "content": {
            "notes": detail.notes,
            "trio": detail.trio.as_deref().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
            "terms": detail.terms,
            "translations": detail.translations,
            "key_points": detail.key_points,
        },
        "transcript": detail
            .segments
            .iter()
            .map(|s| {
                serde_json::json!({
                    "speaker_label": s.speaker_label,
                    "text": s.text,
                    "ts_ms": s.ts_ms,
                    "duration_ms": s.duration_ms,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// 触发会议结束 webhook（配置启用时）：构建 payload 并逐条发送（SSRF 防护）。
/// `proxy` 来自 `NetworkConfig::proxy_url()`；`None` 时直连。
pub fn trigger_meeting_webhooks(
    detail: &SessionDetail,
    cfg: &talksage_config::WebhooksConfig,
    proxy: Option<&str>,
) -> Vec<talksage_core::WebhookResult> {
    if !cfg.enabled || cfg.urls.is_empty() {
        return Vec::new();
    }
    let payload = build_webhook_payload(detail);
    talksage_core::trigger_webhooks(&cfg.urls, &payload, proxy)
}

/// 疑似重复段：同一说话人相邻 final 段文本高度相似（≥0.9）且时间间隔 ≤5s——
/// 通常是 VAD 把同一句话切成两段重复识别（或回环双录同源）。
#[derive(Debug, Clone)]
pub struct DuplicatePair {
    pub idx_a: usize,
    pub idx_b: usize,
    pub speaker: String,
    pub similarity: f32,
    pub gap_ms: u64,
}

/// 在 final 段中检测疑似重复（供会话转储/自动测试/前端标注）。
/// `segments` 的 ts_ms 为段结束时刻（epoch ms）。
pub fn find_duplicate_segments(segments: &[TranscriptSegment]) -> Vec<DuplicatePair> {
    let finals: Vec<&TranscriptSegment> = segments
        .iter()
        .filter(|s| !s.is_partial && !s.text.trim().is_empty())
        .collect();
    let mut out = Vec::new();
    for i in 0..finals.len() {
        for j in (i + 1)..finals.len() {
            let a = finals[i];
            let b = finals[j];
            if a.speaker_id != b.speaker_id {
                continue; // 只查同一说话人（跨流回显由 pipeline 去重处理）
            }
            let gap = b.ts_ms.saturating_sub(a.ts_ms);
            if gap > 5_000 {
                break; // 已超出时间窗（finals 按时间有序），后续更大
            }
            let sim = talksage_core::text_similarity(&a.text, &b.text);
            if sim >= 0.9 {
                out.push(DuplicatePair {
                    idx_a: i,
                    idx_b: j,
                    speaker: a.speaker_label.clone(),
                    similarity: sim,
                    gap_ms: gap,
                });
            }
        }
    }
    out
}

/// 场景模式中文名（导出/展示用；与前端标签一致）。
fn scene_label(mode: &str) -> &'static str {
    match mode {
        "dictation" => "单人听写",
        "conversation" => "一对一会话",
        "translation" => "双语对话",
        "meeting" => "多人会议",
        "lecture" => "演讲/课堂",
        "custom" => "自定义",
        _ => "未知",
    }
}

/// 会话详情导出为 Markdown 单文件（转写 + 纪要 + 指标 + 质量；借鉴 Call.md markdown-export）。
pub fn export_markdown(detail: &SessionDetail) -> String {
    let mut md = String::new();
    // 有会话名就用会话名当标题，#id 与时间退到引用行——导出的文件才认得出是哪场会
    match detail.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => md.push_str(&format!(
            "# {}\n\n> 会议记录 #{}（{}）\n\n",
            title,
            detail.id,
            fmt_unix(detail.started_at)
        )),
        None => md.push_str(&format!(
            "# 会议记录 #{}（{}）\n\n",
            detail.id,
            fmt_unix(detail.started_at)
        )),
    }

    // 概览与指标
    let metrics = talksage_core::compute_conversation_metrics(&detail.segments);
    md.push_str("## 概览\n\n");
    if let Some(meta) = &detail.meta {
        md.push_str(&format!(
            "- 时长 {}s · 语音占比 {:.0}% · 质量 **{}**{}\n",
            meta.duration_ms / 1000,
            meta.speech_ratio * 100.0,
            meta.quality_label(),
            if meta.skipped_analysis { "（跳过下游分析）" } else { "" },
        ));
        // 运行环境：模型/场景/参数快照（对比不同 ASR 配置时用）
        if let Some(ri) = &meta.runtime_info {
            md.push_str(&format!(
                "- 场景 {} · 引擎 {}（{}）· 应用 v{}\n",
                scene_label(&ri.scene_mode),
                ri.user_engine,
                if ri.client_enabled { "双流" } else { "单流" },
                ri.app_version,
            ));
            md.push_str(&format!(
                "- VAD {} · 降噪 {} · 最短提交 {}ms · 增益 {}dB · 说话人 {}\n",
                ri.vad_preset,
                if ri.denoise_enabled { "开" } else { "关" },
                ri.min_segment_ms,
                ri.input_gain_db,
                ri.speaker_mode,
            ));
        }
    }
    if metrics.has_data() {
        md.push_str(&format!(
            "- 发言占比 我 {:.0}% / 客户 {:.0}% · 语速 {:.0} WPM · 提问 {} · 独白 {} · 打断 {} · 健康分 **{}**\n",
            metrics.talk_ratio_me * 100.0,
            metrics.talk_ratio_them * 100.0,
            metrics.pace_wpm,
            metrics.questions_me,
            if metrics.monologue_detected { "是" } else { "否" },
            metrics.interruption_count,
            metrics.health_score,
        ));
    }

    // 纪要
    md.push_str("\n## 会议纪要\n\n");
    match &detail.notes {
        Some(n) => md.push_str(&format!("{}\n", n)),
        None => md.push_str("（未生成）\n"),
    }

    // 智能纪要
    md.push_str("\n## 智能纪要\n\n");
    let trio = detail.trio.as_deref().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    match trio {
        Some(t) => {
            if let Some(o) = t.get("short_overview").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                md.push_str(&format!("**概述**：{o}\n\n"));
            }
            if let Some(kps) = t.get("key_points").and_then(|v| v.as_array()) {
                md.push_str("**关键要点**\n\n");
                for kp in kps {
                    let topic = kp.get("topic").and_then(|v| v.as_str()).unwrap_or("");
                    md.push_str(&format!("### {topic}\n"));
                    if let Some(pts) = kp.get("points").and_then(|v| v.as_array()) {
                        for p in pts {
                            if let Some(s) = p.as_str() {
                                md.push_str(&format!("- {s}\n"));
                            }
                        }
                    }
                    md.push('\n');
                }
            }
            if let Some(items) = t.get("action_items").and_then(|v| v.as_array()) {
                md.push_str("**行动项**\n\n");
                for it in items {
                    if let Some(s) = it.as_str() {
                        md.push_str(&format!("- [ ] {s}\n"));
                    }
                }
                md.push('\n');
            }
        }
        None => md.push_str("（未生成）\n"),
    }

    md.push_str("\n## 会中要点\n\n");
    if detail.key_points.is_empty() {
        md.push_str("（无）\n");
    } else {
        for kp in &detail.key_points {
            md.push_str(&format!("- **{}** {}\n", kp.category.label_zh(), kp.content));
        }
    }

    // 专业术语：会中解释过的词，导出后别人也能看懂这场会在说什么
    if !detail.terms.is_empty() {
        md.push_str("\n## 专业术语\n\n");
        for term in &detail.terms {
            md.push_str(&format!("- {term}\n"));
        }
        md.push('\n');
    }

    // 转写
    md.push_str("## 转写\n\n");
    for s in &detail.segments {
        md.push_str(&format!("**[{}]** {}\n", s.speaker_label, s.text));
    }
    md.push('\n');
    md
}

/// 纯文本转写导出（无 Markdown 标记）：`[说话人] 文本` 逐行，附带开始时间。
/// 用于粘贴进邮件/文档/笔记，或作为语音标注素材。
pub fn export_transcript_text(detail: &SessionDetail) -> String {
    let mut out = String::new();
    out.push_str(&format!("会话 #{}{}\n", detail.id, fmt_unix_line(detail.started_at)));
    out.push_str(&format!("（{} 段）\n\n", detail.segments.len()));
    let origin_ms = detail.started_at.max(0) as u64 * 1000;
    for s in &detail.segments {
        let offset_secs = s.ts_ms.saturating_sub(origin_ms) / 1000;
        let mm = offset_secs / 60;
        let ss = offset_secs % 60;
        out.push_str(&format!("[{:02}:{:02}] [{}] {}\n", mm, ss, s.speaker_label, s.text));
    }
    out
}

/// Unix 秒 → "YYYY-MM-DD HH:MM"（与 [`fmt_unix`] 相同的 civil date 算法，前缀不同）。
fn fmt_unix_line(secs: i64) -> String {
    format!("（开始于 {}）", fmt_unix(secs))
}

/// Unix 秒 → "YYYY-MM-DD HH:MM"（UTC；无 chrono 依赖，Hinnant civil date 算法）。
fn fmt_unix(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", sod / 3600, (sod % 3600) / 60)
}

/// 由首条提问生成话题名：压掉换行、截到 24 个字符。
///
/// 按字符而不是字节截断——中文提问按字节切会把一个字劈成两半。
pub fn auto_thread_title(first_question: &str) -> String {
    let flat = first_question.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title: String = flat.chars().take(24).collect();
    if flat.chars().count() > 24 {
        title.push('…');
    }
    title
}

/// AI 助手话题（左侧列表用）。
#[derive(Debug, Clone, Serialize)]
pub struct ChatThread {
    pub id: i64,
    /// 话题名；None = 尚未命名（首条提问后自动生成）。
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: u64,
}

/// AI 助手的一条消息。
#[derive(Debug, Clone, Serialize)]
pub struct ChatMessageRecord {
    pub id: i64,
    /// user | assistant
    pub role: String,
    pub content: String,
    pub ts_ms: i64,
}

/// 会话概要（历史列表用）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionRecord {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// 用户自定义会话名（None = 未命名，界面回退到 "#id · 时间"）。
    pub title: Option<String>,
    pub segment_count: u64,
    pub term_count: u64,
    /// 会话质量（"clean"/"noise"/"silent"/"low"，老数据为 None）。
    pub quality: Option<String>,
    /// 会话时长（ms）。
    pub duration_ms: Option<u64>,
    /// 语音占比（0..1）。
    pub speech_ratio: Option<f32>,
}

/// 搜索命中（跨会话文本检索）。
#[derive(Debug, Clone, Serialize)]
pub struct SegmentHit {
    pub session_id: i64,
    pub speaker_label: String,
    pub text: String,
    pub ts_ms: u64,
}

/// 会话详情（含全部内容与元数据）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// 用户自定义会话名（None = 未命名）。
    pub title: Option<String>,
    pub segments: Vec<TranscriptSegment>,
    pub terms: Vec<String>,
    pub translations: Vec<String>,
    pub key_points: Vec<KeyPointRecord>,
    pub notes: Option<String>,
    /// 三段式智能纪要（JSON 字符串；借鉴 Call.md summary-generator）。
    pub trio: Option<String>,
    /// 会话元数据（统计/质量），老数据为 None。
    pub meta: Option<SessionMeta>,
}

/// 单条流的统计（写入 meta 用）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamMeta {
    pub speaker_label: String,
    pub total_ms: u64,
    pub speech_ms: u64,
    pub final_segments: usize,
    pub avg_rms: f32,
    pub max_rms: f32,
    /// 非语音块平均 RMS（背景噪音水平）。
    #[serde(default)]
    pub non_speech_avg_rms: f32,
    pub recording: Option<String>,
    pub vad_preset: String,
    pub vad_threshold: f32,
    /// final 段词数（会话指标）。旧数据缺省 0。
    #[serde(default)]
    pub words: usize,
    /// final 段问句数。旧数据缺省 0。
    #[serde(default)]
    pub questions: usize,
}

/// 质量评估参数（阈值可配置；auto_detect 时能量阈值自动计算）。
#[derive(Debug, Clone)]
pub struct QualityParams {
    /// 自动检测背景噪音（非语音块 RMS）并自动设置能量阈值。
    pub auto_detect: bool,
    /// 文本噪音评分阈值（0..1）。
    pub text_noise_threshold: f32,
    /// 静音判定：语音占比低于此值。
    pub min_speech_ratio: f32,
    /// 噪音判定：语音占比高于此值（几乎不停顿）。
    pub max_speech_ratio: f32,
    /// 静音能量阈值。
    pub silence_rms: f32,
    /// 高能量噪音阈值。
    pub high_rms: f32,
}

impl Default for QualityParams {
    fn default() -> Self {
        Self {
            auto_detect: true,
            text_noise_threshold: 0.45,
            min_speech_ratio: 0.15,
            max_speech_ratio: 0.85,
            silence_rms: 0.01,
            high_rms: 0.5,
        }
    }
}

impl QualityParams {
    /// 从 talksage-config 的 QualityConfig 构建。
    pub fn from_config(c: &talksage_config::QualityConfig) -> Self {
        Self {
            auto_detect: c.auto_detect,
            text_noise_threshold: c.text_noise_threshold,
            min_speech_ratio: c.min_speech_ratio,
            max_speech_ratio: c.max_speech_ratio,
            silence_rms: c.silence_rms,
            high_rms: c.high_rms,
        }
    }
}

/// 会话元数据：聚合统计 + 质量评估（存于 sessions.meta JSON）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// 质量结论：clean / noise / silent / low。
    pub quality: String,
    /// 是否跳过下游分析（要点聚合/简报等）。
    pub skipped_analysis: bool,
    /// 会话总时长（ms，主用户流）。
    pub duration_ms: u64,
    /// 语音时长（ms，主用户流）。
    pub speech_ms: u64,
    /// 语音占比（主用户流）。
    pub speech_ratio: f32,
    /// 平均 RMS（主用户流）。
    pub avg_rms: f32,
    /// 峰值 RMS（主用户流）。
    pub max_rms: f32,
    /// 文本噪音评分（各段平均，0..1）。
    pub text_noise: f32,
    /// 面向普通回放的完整会话录音。单流复用原分轨，双流为左右声道主录音。
    #[serde(default)]
    pub master_recording: Option<String>,
    /// 各流明细。
    pub streams: Vec<StreamMeta>,
    /// 采样时刻（Unix 秒）。
    pub evaluated_at: i64,
    /// 运行环境快照（模型/场景/主要参数）：事后可据此对比不同 ASR 配置的
    /// 转写质量，或按相同参数重放历史录音。旧数据缺省 None。
    #[serde(default)]
    pub runtime_info: Option<SessionRuntimeInfo>,
}

/// 会话运行环境快照：当时使用的模型、场景模式与主要参数。
///
/// 用于「事后分析」：把会话转写质量与具体 ASR 配置关联起来，对比不同引擎/
/// 参数（paraformer vs whisper vs qwen3、VAD 灵敏度、降噪开关…）的效果，
/// 或按相同配置重放历史录音复现/优化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRuntimeInfo {
    /// 应用版本（如 "0.1.1"）。
    pub app_version: String,
    /// 场景模式（dictation / conversation / translation / meeting / lecture / custom）。
    pub scene_mode: String,
    /// 用户流引擎（paraformer-zh / zipformer-en / whisper-base / whisper-small / qwen3-asr）。
    pub user_engine: String,
    /// 客户流引擎（None = 单流）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_engine: Option<String>,
    /// 是否双流。
    #[serde(default)]
    pub client_enabled: bool,
    /// VAD 灵敏度预设（sensitive / standard / strict）。
    pub vad_preset: String,
    /// VAD 检测阈值。
    pub vad_threshold: f32,
    /// VAD 段尾静音（ms；覆盖时）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_min_silence_ms: Option<u64>,
    /// 降噪开关。
    pub denoise_enabled: bool,
    /// 最短提交时长（ms）。
    pub min_segment_ms: u64,
    /// 输入增益（dB）。
    pub input_gain_db: f32,
    /// 说话人识别模式（off / channel / voiceprint）。
    pub speaker_mode: String,
    /// 采样率（一般 16000）。
    pub sample_rate: u32,
}

impl SessionMeta {
    /// 质量中文标签（前端展示）。
    pub fn quality_label(&self) -> &'static str {
        match self.quality.as_str() {
            "clean" => "正常",
            "noise" => "噪音",
            "silent" => "静音",
            "low" => "待复核",
            _ => "未知",
        }
    }

    /// 根据各流统计 + 段文本评估会话质量。
    ///
    /// 规则（主用户流"我"为准，阈值来自 `params`）：
    /// - 时长 < 2s → low
    /// - 无语音（speech_ms==0）：能量 < silence_rms → silent；有能量（环境噪音但 VAD 不认）→ noise
    /// - 语音占比 < min_speech_ratio → silent（有语音但极少）
    /// - 语音占比 < 0.4 或平均 RMS > high_rms → noise（语音少 / 环境能量大）
    /// - 语音占比 > max_speech_ratio → noise（几乎无停顿，持续有声：噪音/音乐/旁人说话，VAD 误判为语音）
    /// - 文本噪音评分 > text_noise_threshold → noise（VAD 认为是语音，但内容是重复/语气词噪音）
    /// - 语音占比 < 0.6 → low（待复核）
    /// - 否则 → clean
    ///
    /// `params.auto_detect = true` 时，silence_rms / high_rms 用会话中非语音块的
    /// 背景噪音水平自动计算（覆盖手工设定值）。
    pub fn evaluate(stats: Vec<StreamMeta>, segment_texts: &[String], now: i64, params: &QualityParams) -> Self {
        let main = stats
            .iter()
            .max_by_key(|s| s.total_ms)
            .cloned()
            .unwrap_or_else(|| StreamMeta {
                speaker_label: "我".into(),
                total_ms: 0,
                speech_ms: 0,
                final_segments: 0,
                avg_rms: 0.0,
                max_rms: 0.0,
                non_speech_avg_rms: 0.0,
                recording: None,
                vad_preset: String::new(),
                vad_threshold: 0.0,
                ..Default::default()
            });
        let ratio = if main.total_ms > 0 {
            main.speech_ms as f32 / main.total_ms as f32
        } else {
            0.0
        };
        let text_noise = if segment_texts.is_empty() {
            0.0
        } else {
            segment_texts
                .iter()
                .map(|t| talksage_core::text_noise_score(t))
                .sum::<f32>()
                / segment_texts.len() as f32
        };

        // 自动检测背景噪音 → 自动能量阈值（背景之上 1.5 倍才算语音，5 倍背景/最低 0.2 算高能量）
        let (silence_rms, high_rms) = if params.auto_detect {
            let bg = main.non_speech_avg_rms.max(0.0);
            ((bg * 1.5).max(0.001), (bg * 5.0).max(0.2))
        } else {
            (params.silence_rms, params.high_rms)
        };

        let quality = if main.total_ms < 2000 {
            talksage_core::SessionQuality::Low
        } else if main.speech_ms == 0 {
            // 无语音：能量低 = 静音；有能量 = 环境噪音（VAD 不认为是语音）
            if main.avg_rms < silence_rms {
                talksage_core::SessionQuality::Silent
            } else {
                talksage_core::SessionQuality::Noise
            }
        } else if ratio < params.min_speech_ratio {
            talksage_core::SessionQuality::Silent
        } else if ratio < 0.4 || main.avg_rms > high_rms || ratio > params.max_speech_ratio || text_noise > params.text_noise_threshold {
            talksage_core::SessionQuality::Noise
        } else if ratio < 0.6 {
            talksage_core::SessionQuality::Low
        } else {
            talksage_core::SessionQuality::Clean
        };

        let quality_str = match quality {
            talksage_core::SessionQuality::Clean => "clean",
            talksage_core::SessionQuality::Noise => "noise",
            talksage_core::SessionQuality::Silent => "silent",
            talksage_core::SessionQuality::Low => "low",
        }
        .to_string();

        SessionMeta {
            quality: quality_str,
            skipped_analysis: quality.skip_analysis(),
            duration_ms: main.total_ms,
            speech_ms: main.speech_ms,
            speech_ratio: ratio,
            avg_rms: main.avg_rms,
            max_rms: main.max_rms,
            text_noise,
            master_recording: None,
            streams: stats,
            evaluated_at: now,
            runtime_info: None, // 由调用方（QualityHost）在写入前填充
        }
    }
}

/// 会话存储（线程安全）。
pub struct SessionStore {
    conn: Mutex<Connection>,
}

impl SessionStore {
    /// 打开/创建数据库（`path` 为 `:memory:` 时用内存库，测试用）。
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        if path != ":memory:" {
            if let Some(parent) = Path::new(path).parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                notes TEXT,
                meta TEXT,
                trio TEXT,
                title TEXT
            );
            CREATE TABLE IF NOT EXISTS segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                speaker_id INTEGER NOT NULL,
                speaker_label TEXT NOT NULL,
                text TEXT NOT NULL,
                ts_ms INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                rms REAL NOT NULL DEFAULT 0,
                speaker_attribution TEXT,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS terms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS translations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                direction TEXT NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS key_points (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                result_id TEXT NOT NULL,
                category TEXT NOT NULL,
                content TEXT NOT NULL,
                ts_ms INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS chat_threads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chat_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                thread_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                ts_ms INTEGER NOT NULL,
                FOREIGN KEY(thread_id) REFERENCES chat_threads(id)
            );
            CREATE INDEX IF NOT EXISTS idx_chat_messages_thread ON chat_messages(thread_id);
            CREATE INDEX IF NOT EXISTS idx_segments_text ON segments(text);
            CREATE INDEX IF NOT EXISTS idx_segments_session ON segments(session_id);
            CREATE INDEX IF NOT EXISTS idx_terms_session ON terms(session_id);
            CREATE INDEX IF NOT EXISTS idx_translations_session ON translations(session_id);
            CREATE INDEX IF NOT EXISTS idx_key_points_session ON key_points(session_id);
            ",
        )?;
        // 迁移（旧库）：sessions.meta / sessions.trio / sessions.title / segments.duration_ms / segments.rms
        let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN meta TEXT;");
        let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN title TEXT;");
        let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN trio TEXT;");
        let _ = conn.execute_batch("ALTER TABLE segments ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;");
        let _ = conn.execute_batch("ALTER TABLE segments ADD COLUMN rms REAL NOT NULL DEFAULT 0;");
        let _ = conn.execute_batch("ALTER TABLE segments ADD COLUMN speaker_attribution TEXT;");
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 开启新会话，返回 session id。
    pub fn start_session(&self, started_at: i64) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO sessions (started_at) VALUES (?1)", [started_at])?;
        Ok(conn.last_insert_rowid())
    }

    /// 结束会话。
    pub fn end_session(&self, session_id: i64, ended_at: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, ended_at],
        )?;
        Ok(())
    }

    /// 启动恢复：收尾上次异常退出遗留的「未结束」会话（`ended_at IS NULL`）。
    ///
    /// 结束时刻按该会话最后一段转写的结束时间推算（`started_at` 秒 + 段内毫秒偏移
    /// 向上取整）；没有任何段则退回 `started_at`。返回被收尾的会话 id。
    /// 幂等：已有 `ended_at` 的会话不受影响。
    pub fn close_orphan_sessions(&self) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.started_at,
                    COALESCE(MAX(g.ts_ms + g.duration_ms), 0)
             FROM sessions s
             LEFT JOIN segments g ON g.session_id = s.id
             WHERE s.ended_at IS NULL
             GROUP BY s.id, s.started_at",
        )?;
        let rows: Vec<(i64, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        drop(stmt);

        let mut closed = Vec::new();
        for (id, started_at, last_ms) in rows {
            // 毫秒偏移向上取整成秒，保证 ended_at >= started_at 且不丢掉不足 1s 的尾巴
            let ended_at = started_at + (last_ms + 999) / 1000;
            conn.execute(
                "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
                rusqlite::params![id, ended_at],
            )?;
            closed.push(id);
        }
        closed.sort();
        Ok(closed)
    }

    /// 追加转写段（含时长/能量统计）。
    pub fn add_segment(&self, session_id: i64, seg: &TranscriptSegment) -> Result<()> {
        let attribution = seg
            .speaker_attribution
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO segments (session_id, speaker_id, speaker_label, text, ts_ms, duration_ms, rms, speaker_attribution)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                session_id,
                seg.speaker_id,
                seg.speaker_label,
                seg.text,
                seg.ts_ms,
                seg.duration_ms,
                seg.rms,
                attribution
            ],
        )?;
        Ok(())
    }

    /// 保存会话元数据（统计 + 质量评估 JSON）。
    pub fn set_session_meta(&self, session_id: i64, meta: &SessionMeta) -> Result<()> {
        let json = serde_json::to_string(meta)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET meta = ?2 WHERE id = ?1",
            rusqlite::params![session_id, json],
        )?;
        Ok(())
    }

    /// 追加术语。
    pub fn add_term(&self, session_id: i64, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO terms (session_id, content) VALUES (?1,?2)",
            rusqlite::params![session_id, content],
        )?;
        Ok(())
    }

    /// 追加翻译。
    pub fn add_translation(&self, session_id: i64, direction: &str, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO translations (session_id, direction, content) VALUES (?1,?2,?3)",
            rusqlite::params![session_id, direction, content],
        )?;
        Ok(())
    }

    /// 追加会中要点。
    pub fn add_key_point(&self, session_id: i64, kp: &KeyPointRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO key_points (session_id, result_id, category, content, ts_ms) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![session_id, kp.result_id, kp.category.as_str(), kp.content, kp.ts_ms],
        )?;
        Ok(())
    }

    /// 保存纪要。
    pub fn set_notes(&self, session_id: i64, notes: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET notes = ?2 WHERE id = ?1",
            rusqlite::params![session_id, notes],
        )?;
        Ok(())
    }

    /// 重命名会话。空串（或全空白）→ 存 NULL，等于清除自定义名、回到"#id · 时间"。
    pub fn set_session_title(&self, session_id: i64, title: &str) -> Result<()> {
        let trimmed = title.trim();
        let value = if trimmed.is_empty() { None } else { Some(trimmed) };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET title = ?2 WHERE id = ?1",
            rusqlite::params![session_id, value],
        )?;
        Ok(())
    }

    /// 保存三段式智能纪要（JSON 字符串）。
    pub fn set_trio(&self, session_id: i64, trio: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET trio = ?2 WHERE id = ?1",
            rusqlite::params![session_id, trio],
        )?;
        Ok(())
    }

    /// 删除会话及其全部关联数据（段/术语/翻译/要点）。
    pub fn delete_session(&self, session_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM segments WHERE session_id = ?1", [session_id])?;
        conn.execute("DELETE FROM terms WHERE session_id = ?1", [session_id])?;
        conn.execute("DELETE FROM translations WHERE session_id = ?1", [session_id])?;
        conn.execute("DELETE FROM key_points WHERE session_id = ?1", [session_id])?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", [session_id])?;
        Ok(())
    }

    /// 会话列表（按时间倒序）。
    pub fn list_sessions(&self, limit: u32) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.started_at, s.ended_at,
                    (SELECT COUNT(*) FROM segments g WHERE g.session_id = s.id),
                    (SELECT COUNT(*) FROM terms t WHERE t.session_id = s.id),
                    s.meta, s.title
             FROM sessions s ORDER BY s.id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                let meta_raw: Option<String> = r.get(5)?;
                let meta = meta_raw.as_deref().and_then(SessionMeta::from_json);
                Ok(SessionRecord {
                    id: r.get(0)?,
                    started_at: r.get(1)?,
                    ended_at: r.get(2)?,
                    title: r.get(6)?,
                    segment_count: r.get(3)?,
                    term_count: r.get(4)?,
                    quality: meta.as_ref().map(|m| m.quality.clone()),
                    duration_ms: meta.as_ref().map(|m| m.duration_ms),
                    speech_ratio: meta.as_ref().map(|m| m.speech_ratio),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── AI 助手（chat）────────────────────────────────────────────────

    /// 新建话题，返回 id。标题留空，等首条提问后由 [`Self::add_chat_message`] 自动生成。
    pub fn create_chat_thread(&self, now: i64) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO chat_threads (title, created_at, updated_at) VALUES (NULL, ?1, ?1)",
            [now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 话题列表（最近活跃在前）。
    pub fn list_chat_threads(&self, limit: u32) -> Result<Vec<ChatThread>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.title, t.created_at, t.updated_at,
                    (SELECT COUNT(*) FROM chat_messages m WHERE m.thread_id = t.id)
             FROM chat_threads t ORDER BY t.updated_at DESC, t.id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok(ChatThread {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    message_count: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 话题内的全部消息（按时间升序）。
    pub fn get_chat_messages(&self, thread_id: i64) -> Result<Vec<ChatMessageRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, role, content, ts_ms FROM chat_messages WHERE thread_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([thread_id], |r| {
                Ok(ChatMessageRecord {
                    id: r.get(0)?,
                    role: r.get(1)?,
                    content: r.get(2)?,
                    ts_ms: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 追加一条消息，返回消息 id。
    ///
    /// 同时把话题的 `updated_at` 推到最新；话题还没有标题时，用首条用户提问
    /// 截出一个（列表里总得有个能认出来的名字）。
    pub fn add_chat_message(&self, thread_id: i64, role: &str, content: &str, ts_ms: i64) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO chat_messages (thread_id, role, content, ts_ms) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![thread_id, role, content, ts_ms],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE chat_threads SET updated_at = ?2 WHERE id = ?1",
            rusqlite::params![thread_id, ts_ms / 1000],
        )?;
        if role == "user" {
            conn.execute(
                "UPDATE chat_threads SET title = ?2 WHERE id = ?1 AND (title IS NULL OR title = '')",
                rusqlite::params![thread_id, auto_thread_title(content)],
            )?;
        }
        Ok(id)
    }

    /// 覆盖某条消息的正文（流式回答边生成边落库）。
    pub fn update_chat_message(&self, message_id: i64, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chat_messages SET content = ?2 WHERE id = ?1",
            rusqlite::params![message_id, content],
        )?;
        Ok(())
    }

    /// 删除单条消息（生成失败、一个字都没出的空占位）。
    pub fn delete_chat_message(&self, message_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM chat_messages WHERE id = ?1", [message_id])?;
        Ok(())
    }

    /// 重命名话题；空串 = 清除自定义名（下次提问会重新自动生成）。
    pub fn set_chat_thread_title(&self, thread_id: i64, title: &str) -> Result<()> {
        let trimmed = title.trim();
        let value = if trimmed.is_empty() { None } else { Some(trimmed) };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chat_threads SET title = ?2 WHERE id = ?1",
            rusqlite::params![thread_id, value],
        )?;
        Ok(())
    }

    /// 删除话题及其全部消息。
    pub fn delete_chat_thread(&self, thread_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM chat_messages WHERE thread_id = ?1", [thread_id])?;
        conn.execute("DELETE FROM chat_threads WHERE id = ?1", [thread_id])?;
        Ok(())
    }

    /// 全文检索转写段。
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SegmentHit>> {
        let conn = self.conn.lock().unwrap();
        let q = format!("%{}%", query.trim());
        let mut stmt = conn.prepare(
            "SELECT session_id, speaker_label, text, ts_ms FROM segments
             WHERE text LIKE ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![q, limit], |r| {
                Ok(SegmentHit {
                    session_id: r.get(0)?,
                    speaker_label: r.get(1)?,
                    text: r.get(2)?,
                    ts_ms: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 会话详情。
    pub fn get_session(&self, session_id: i64) -> Result<SessionDetail> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, started_at, ended_at, notes, meta, trio, title FROM sessions WHERE id = ?1",
                [session_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("会话不存在: {session_id}"))?;
        let (id, started_at, ended_at, notes, meta_raw, trio, title) = row;

        let segments = {
            let mut stmt = conn.prepare(
                "SELECT speaker_id, speaker_label, text, ts_ms, duration_ms, rms, speaker_attribution FROM segments WHERE session_id = ?1 ORDER BY id",
            )?;
            let rows = stmt
                .query_map([session_id], |r| {
                    let speaker_id = r.get(0)?;
                    let speaker_label: String = r.get(1)?;
                    let raw: Option<String> = r.get(6)?;
                    let speaker_attribution = raw
                        .as_deref()
                        .and_then(|json| serde_json::from_str(json).ok())
                        .or_else(|| {
                            Some(talksage_core::SpeakerAttribution::from_legacy(
                                talksage_core::AudioSource::Unknown,
                                &speaker_label,
                            ))
                        });
                    Ok(TranscriptSegment {
                        speaker_id,
                        speaker_label,
                        speaker_attribution,
                        text: r.get(2)?,
                        is_partial: false,
                        ts_ms: r.get(3)?,
                        duration_ms: r.get(4)?,
                        rms: r.get(5)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let terms: Vec<String> = {
            let mut stmt = conn.prepare("SELECT content FROM terms WHERE session_id = ?1 ORDER BY id")?;
            let rows = stmt
                .query_map([session_id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let translations: Vec<String> = {
            let mut stmt = conn.prepare("SELECT content FROM translations WHERE session_id = ?1 ORDER BY id")?;
            let rows = stmt
                .query_map([session_id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let key_points: Vec<KeyPointRecord> = {
            let mut stmt = conn.prepare(
                "SELECT result_id, category, content, ts_ms FROM key_points WHERE session_id = ?1 ORDER BY id",
            )?;
            let rows = stmt
                .query_map([session_id], |r| {
                    let category_raw: String = r.get(1)?;
                    Ok(KeyPointRecord {
                        result_id: r.get(0)?,
                        category: talksage_core::KeyPointCategory::from_name(&category_raw),
                        content: r.get(2)?,
                        ts_ms: r.get(3)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        Ok(SessionDetail {
            id,
            started_at,
            ended_at,
            title,
            segments,
            terms,
            translations,
            key_points,
            notes,
            trio,
            meta: meta_raw.as_deref().and_then(SessionMeta::from_json),
        })
    }
}

impl SessionMeta {
    /// 从 JSON 解析（容错：损坏返回 None）。
    pub fn from_json(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SessionStore {
        SessionStore::open(":memory:").unwrap()
    }

    fn seg(speaker: u32, label: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: speaker,
            speaker_label: label.into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms: 1000,
            duration_ms: 800,
            rms: 0.2,
        }
    }

    /// 带时间戳/时长的段（重复检测测试用）。
    fn seg_at(speaker: u32, label: &str, text: &str, ts_ms: u64, duration_ms: u64) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: speaker,
            speaker_label: label.into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms,
            duration_ms,
            rms: 0.2,
        }
    }

    /// 崩溃后 `ended_at` 为 NULL 的会话应在启动时收尾，
    /// 否则历史里会永远挂着一条「进行中」的僵尸会话。
    #[test]
    fn close_orphan_sessions_uses_last_segment_time() {
        let s = store();
        let crashed = s.start_session(1_000).unwrap();
        s.add_segment(crashed, &seg_at(0, "我", "崩溃前最后一句", 5_000, 800)).unwrap();
        let normal = s.start_session(2_000).unwrap();
        s.end_session(normal, 3_000).unwrap();

        let closed = s.close_orphan_sessions().unwrap();
        assert_eq!(closed, vec![crashed], "只应收尾未结束的会话");

        // 崩溃会话按最后一段的结束时刻收尾：1000 + ceil((5000+800)/1000) = 1006
        let d = s.get_session(crashed).unwrap();
        assert_eq!(d.ended_at, Some(1_006), "应按最后一段推算结束时间");
        // 已正常结束的不受影响
        assert_eq!(s.get_session(normal).unwrap().ended_at, Some(3_000));

        // 幂等：再跑一次没有可收尾的
        assert!(s.close_orphan_sessions().unwrap().is_empty());
    }

    /// 没有任何转写段的崩溃会话，退回到 `started_at`（时长 0）。
    #[test]
    fn close_orphan_session_without_segments_falls_back_to_start() {
        let s = store();
        let empty = s.start_session(7_000).unwrap();
        assert_eq!(s.close_orphan_sessions().unwrap(), vec![empty]);
        assert_eq!(s.get_session(empty).unwrap().ended_at, Some(7_000));
    }

    #[test]
    fn crud_roundtrip() {
        let s = store();
        let id = s.start_session(111).unwrap();
        let mut client = seg(1, "客户", "We need NPI samples");
        client.speaker_attribution = Some(talksage_core::SpeakerAttribution {
            source: talksage_core::AudioSource::SystemLoopback,
            role: talksage_core::SpeakerRole::Client,
            voice: Some(talksage_core::VoiceIdentity {
                id: "客户1".into(),
                confidence: Some(0.82),
            }),
        });
        s.add_segment(id, &client).unwrap();
        s.add_segment(id, &seg(0, "我", "好的")).unwrap();
        s.add_term(id, "NPI = 新产品导入").unwrap();
        s.add_translation(id, "en_zh", "我们需要 NPI 样品").unwrap();
        s.end_session(id, 222).unwrap();

        let detail = s.get_session(id).unwrap();
        assert_eq!(detail.segments.len(), 2);
        assert_eq!(detail.segments[0].duration_ms, 800);
        assert!((detail.segments[0].rms - 0.2).abs() < 1e-6);
        let attribution = detail.segments[0].speaker_attribution.as_ref().unwrap();
        assert_eq!(attribution.source, talksage_core::AudioSource::SystemLoopback);
        assert_eq!(attribution.voice.as_ref().unwrap().id, "客户1");
        // 旧式调用未提供 attribution，读取时仍按 label 推导兼容角色。
        assert_eq!(
            detail.segments[1].speaker_attribution.as_ref().unwrap().role,
            talksage_core::SpeakerRole::Owner
        );
        assert_eq!(detail.terms, vec!["NPI = 新产品导入"]);
        assert_eq!(detail.translations.len(), 1);
        assert_eq!(detail.started_at, 111);
        assert_eq!(detail.ended_at, Some(222));
    }

    #[test]
    fn key_points_persist_and_appear_in_detail() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_key_point(
            id,
            &KeyPointRecord {
                result_id: "kp-1".into(),
                category: talksage_core::KeyPointCategory::Requirement,
                content: "We need NPI samples by Friday.".into(),
                ts_ms: 1000,
            },
        )
        .unwrap();
        let detail = s.get_session(id).unwrap();
        assert_eq!(detail.key_points.len(), 1);
        assert_eq!(detail.key_points[0].category, talksage_core::KeyPointCategory::Requirement);
        assert!(detail.key_points[0].content.contains("NPI"));
        let md = export_markdown(&detail);
        assert!(md.contains("会中要点"));
        assert!(md.contains("要求"));
    }

    #[test]
    fn old_database_schema_migrates_speaker_attribution_column() {
        let path = std::env::temp_dir().join(format!(
            "talksage-old-schema-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id INTEGER PRIMARY KEY AUTOINCREMENT, started_at INTEGER NOT NULL, ended_at INTEGER, notes TEXT);
                 CREATE TABLE segments (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER NOT NULL, speaker_id INTEGER NOT NULL, speaker_label TEXT NOT NULL, text TEXT NOT NULL, ts_ms INTEGER NOT NULL);
                 CREATE TABLE terms (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER NOT NULL, content TEXT NOT NULL);
                 CREATE TABLE translations (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER NOT NULL, direction TEXT NOT NULL, content TEXT NOT NULL);",
            )
            .unwrap();
        }

        let store = SessionStore::open(&path.to_string_lossy()).unwrap();
        let id = store.start_session(1).unwrap();
        let mut segment = seg(0, "我", "迁移后写入");
        segment.speaker_attribution = Some(talksage_core::SpeakerAttribution {
            source: talksage_core::AudioSource::Microphone,
            role: talksage_core::SpeakerRole::Owner,
            voice: None,
        });
        store.add_segment(id, &segment).unwrap();
        let detail = store.get_session(id).unwrap();
        assert_eq!(
            detail.segments[0].speaker_attribution.as_ref().unwrap().source,
            talksage_core::AudioSource::Microphone
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    /// 会话命名：列表与详情都要带出名字，空串等于"取消命名"退回默认显示。
    #[test]
    fn rename_session_roundtrip() {
        let s = store();
        let id = s.start_session(1).unwrap();
        assert_eq!(s.get_session(id).unwrap().title, None, "新会话默认未命名");

        s.set_session_title(id, "  周三 NPI 评审  ").unwrap();
        assert_eq!(s.get_session(id).unwrap().title.as_deref(), Some("周三 NPI 评审"), "应去掉首尾空白");
        let listed = s.list_sessions(10).unwrap();
        assert_eq!(listed[0].title.as_deref(), Some("周三 NPI 评审"), "列表也要带出会话名");

        // 空串 / 纯空白 = 清除自定义名，而不是存一个空标题
        s.set_session_title(id, "   ").unwrap();
        assert_eq!(s.get_session(id).unwrap().title, None);
        assert_eq!(s.list_sessions(10).unwrap()[0].title, None);
    }

    /// 命名后导出的 Markdown 以会话名作标题，未命名时保持原来的 "#id" 标题。
    #[test]
    fn export_markdown_uses_title_when_named() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(0, "我", "开场")).unwrap();

        let plain = export_markdown(&s.get_session(id).unwrap());
        assert!(plain.starts_with("# 会议记录 #"), "未命名时标题不变: {plain}");

        s.set_session_title(id, "周三 NPI 评审").unwrap();
        let named = export_markdown(&s.get_session(id).unwrap());
        assert!(named.starts_with("# 周三 NPI 评审"), "命名后应以会话名开头: {named}");
        assert!(named.contains(&format!("> 会议记录 #{id}")), "编号与时间退到引用行: {named}");
    }

    /// 话题 + 消息往返：列表按最近活跃排序，首条提问自动成为标题。
    #[test]
    fn chat_threads_and_messages_roundtrip() {
        let s = store();
        let t1 = s.create_chat_thread(1_000).unwrap();
        let t2 = s.create_chat_thread(1_001).unwrap();

        assert_eq!(s.list_chat_threads(10).unwrap()[0].id, t2, "新建话题排在最前");
        assert!(s.list_chat_threads(10).unwrap().iter().all(|t| t.title.is_none()));

        s.add_chat_message(t1, "user", "帮我把上周的 NPI 评审整理成行动项", 5_000_000).unwrap();
        let assistant = s.add_chat_message(t1, "assistant", "", 5_000_100).unwrap();
        s.update_chat_message(assistant, "1. 确认样品交期").unwrap();

        let threads = s.list_chat_threads(10).unwrap();
        assert_eq!(threads[0].id, t1, "有新消息的话题排到最前");
        assert_eq!(threads[0].message_count, 2);
        assert_eq!(threads[0].title.as_deref(), Some("帮我把上周的 NPI 评审整理成行动项"));

        let msgs = s.get_chat_messages(t1).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].content, "1. 确认样品交期", "流式回答落库后应是完整文本");

        // 手动改名后，后续提问不再覆盖标题
        s.set_chat_thread_title(t1, "NPI 评审").unwrap();
        s.add_chat_message(t1, "user", "再补一条", 5_000_200).unwrap();
        assert_eq!(s.list_chat_threads(10).unwrap()[0].title.as_deref(), Some("NPI 评审"));

        // 空串清除自定义名
        s.set_chat_thread_title(t1, "  ").unwrap();
        assert_eq!(s.list_chat_threads(10).unwrap()[0].title, None);

        s.delete_chat_thread(t1).unwrap();
        assert!(s.get_chat_messages(t1).unwrap().is_empty(), "消息应随话题一起删除");
        assert_eq!(s.list_chat_threads(10).unwrap().len(), 1);
    }

    /// 自动标题按字符截断——中文按字节切会劈坏字。
    #[test]
    fn auto_thread_title_truncates_by_chars() {
        assert_eq!(auto_thread_title("短问题"), "短问题");
        let long = "一二三四五六七八九十一二三四五六七八九十一二三四五";
        let title = auto_thread_title(long);
        assert_eq!(title.chars().count(), 25, "24 字 + 省略号");
        assert!(title.ends_with('…'));
        assert_eq!(auto_thread_title("多行\n提问  带空白"), "多行 提问 带空白");
    }

    #[test]
    fn list_and_search() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(1, "客户", "The MOQ is 1000 units")).unwrap();
        s.add_segment(id, &seg(0, "我", "我们确认")).unwrap();

        let list = s.list_sessions(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].segment_count, 2);

        let hits = s.search("MOQ", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("MOQ"));

        let none = s.search("不存在词", 10).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn get_missing_session_errors() {
        let s = store();
        assert!(s.get_session(999).is_err());
    }

    #[test]
    fn notes_save_and_retrieve() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(1, "客户", "hello")).unwrap();
        s.set_notes(id, "# 会议纪要\n\n## 摘要\ntest").unwrap();
        let detail = s.get_session(id).unwrap();
        assert_eq!(detail.notes.as_deref(), Some("# 会议纪要\n\n## 摘要\ntest"));
    }

    #[test]
    fn trio_save_and_retrieve() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(0, "我", "我们需要确认方案")).unwrap();
        let trio = r#"{"short_overview":"概述","key_points":[{"topic":"方案","points":["我确认了方案"]}],"action_items":["客户发邮件确认"]}"#;
        s.set_trio(id, trio).unwrap();
        let detail = s.get_session(id).unwrap();
        assert_eq!(detail.trio.as_deref(), Some(trio));
    }

    #[test]
    fn export_markdown_bundles_all_sections() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(1, "客户", "We need NPI samples by Friday.")).unwrap();
        s.add_segment(id, &seg(0, "我", "我们确认可以安排。")).unwrap();
        s.set_notes(id, "# 会议纪要\n\n## 摘要\n测试").unwrap();
        s.add_term(id, "MOQ：最小起订量，供应商单次接单的最低数量门槛。").unwrap();
        s.set_trio(
            id,
            r#"{"short_overview":"概述文本","key_points":[{"topic":"交付","points":["客户确认了周五交付"]}],"action_items":["客户发邮件确认"]}"#,
        )
        .unwrap();
        s.end_session(id, 1000).unwrap();
        let detail = s.get_session(id).unwrap();

        let md = export_markdown(&detail);
        assert!(md.contains("# 会议记录"), "缺少标题: {md}");
        assert!(md.contains("## 概览"));
        assert!(md.contains("## 会议纪要"));
        assert!(md.contains("## 智能纪要"));
        assert!(md.contains("**概述**：概述文本"));
        assert!(md.contains("### 交付"));
        assert!(md.contains("- [ ] 客户发邮件确认"), "行动项应为可勾选列表: {md}");
        // 会中解释过的术语要跟着导出走：库里有、界面看得到，导出给别人时不能缺
        assert!(md.contains("## 专业术语"), "缺少专业术语小节: {md}");
        assert!(md.contains("- MOQ：最小起订量，供应商单次接单的最低数量门槛。"));
        assert!(md.contains("## 转写"));
        assert!(md.contains("[客户]"));
        assert!(md.contains("We need NPI samples by Friday."));
        assert!(md.contains("[我]"));
        assert!(md.contains("我们确认可以安排。"));
    }

    #[test]
    fn export_transcript_text_plain_lines_with_timestamps() {
        let s = store();
        let id = s.start_session(0).unwrap();
        s.add_segment(id, &seg(0, "我", "大家好")).unwrap();
        s.add_segment(id, &seg(1, "客户", "请介绍方案")).unwrap();
        s.end_session(id, 1000).unwrap();
        let detail = s.get_session(id).unwrap();

        let text = export_transcript_text(&detail);
        assert!(text.contains("[00:0"), "应有相对时间戳 mm:ss: {text}");
        assert!(text.contains("[我] 大家好"), "应含说话人与文本: {text}");
        assert!(text.contains("[客户] 请介绍方案"), "应含第二段: {text}");
        assert!(!text.contains("**"), "纯文本不应含 Markdown 标记: {text}");
        assert!(!text.contains("##"), "纯文本不应含标题标记: {text}");
    }

    #[test]
    fn webhook_payload_includes_meeting_metrics_and_transcript() {
        let s = store();
        let id = s.start_session(100).unwrap();
        s.add_segment(id, &seg(0, "我", "这个价格能再低一些吗？")).unwrap();
        s.add_segment(id, &seg(1, "客户", "可以谈")).unwrap();
        s.end_session(id, 300).unwrap();
        let detail = s.get_session(id).unwrap();

        let payload = build_webhook_payload(&detail);
        assert_eq!(payload["meeting"]["id"].as_i64(), Some(id));
        assert_eq!(payload["meeting"]["duration_seconds"].as_i64(), Some(200));
        assert!(payload["metrics"]["questions_me"].as_u64().unwrap_or(0) >= 1, "问句应入 payload: {payload}");
        assert_eq!(payload["transcript"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn find_duplicate_segments_detects_same_speaker_repeats() {
        // 同说话人相邻段完全相同 → 检出；不同说话人/时间超窗/内容不同 → 不检出
        let segs = vec![
            seg_at(0, "我", "我们确认下周一交付", 2000, 2000),
            seg_at(0, "我", "我们确认下周一交付", 4500, 2000), // 相同 + 间隔 0.5s → 重复
            seg_at(1, "客户", "可以可以", 6000, 1500),          // 不同说话人 → 不参与
            seg_at(0, "我", "我们确认下周一交付", 14500, 2000), // 间隔 10s > 5s → 不检出
            seg_at(0, "我", "客户要求周五前报价", 15500, 2000), // 内容不同 → 不检出
        ];
        let dups = find_duplicate_segments(&segs);
        assert_eq!(dups.len(), 1, "应只检出第 0/1 对: {dups:?}");
        assert_eq!(dups[0].idx_a, 0);
        assert_eq!(dups[0].idx_b, 1);
        assert!(dups[0].similarity >= 0.99);
    }

    #[test]
    fn find_duplicate_segments_empty_for_clean_conversation() {
        let segs = vec![
            seg_at(0, "我", "这个方案我们确认没问题", 2000, 2000),
            seg_at(1, "客户", "好的我们下周一签合同", 5000, 2000),
            seg_at(0, "我", "那就按这个推进", 8000, 1500),
        ];
        assert!(find_duplicate_segments(&segs).is_empty());
    }

    #[test]
    fn delete_session_removes_all_related_rows() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(1, "客户", "We need NPI")).unwrap();
        s.add_segment(id, &seg(0, "我", "好的")).unwrap();
        s.add_term(id, "NPI = 新产品导入").unwrap();
        s.add_translation(id, "en_zh", "我们需要 NPI").unwrap();
        s.add_key_point(
            id,
            &KeyPointRecord {
                result_id: "kp-1".into(),
                category: talksage_core::KeyPointCategory::Requirement,
                content: "Need NPI".into(),
                ts_ms: 1,
            },
        )
        .unwrap();
        s.set_notes(id, "纪要内容").unwrap();

        // 删除后：详情查询报错、列表为空
        s.delete_session(id).unwrap();
        assert!(s.get_session(id).is_err());
        let list = s.list_sessions(10).unwrap();
        assert!(list.is_empty());

        // 不存在的会话删除不报错（幂等）
        s.delete_session(999).unwrap();
    }

    #[test]
    fn session_meta_persists_and_lists_quality() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(0, "我", "我们需要在周五之前拿到 NPI 样品")).unwrap();
        s.add_segment(id, &seg(0, "我", "另外请确认交期")).unwrap();
        let meta = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 60000,
                speech_ms: 45000,
                final_segments: 2,
                avg_rms: 0.12,
                max_rms: 0.6,
                non_speech_avg_rms: 0.02,
                recording: Some("2026-08-19_05-57-20_我.wav".into()),
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
                ..Default::default()
            }],
            &["我们需要在周五之前拿到 NPI 样品".into(), "另外请确认交期".into()],
            12345,
            &QualityParams::default(),
        );
        assert_eq!(meta.quality, "clean");
        assert!(!meta.skipped_analysis);
        s.set_session_meta(id, &meta).unwrap();
        s.end_session(id, 2).unwrap();

        let list = s.list_sessions(10).unwrap();
        assert_eq!(list[0].quality.as_deref(), Some("clean"));
        assert_eq!(list[0].duration_ms, Some(60000));
        assert!((list[0].speech_ratio.unwrap() - 0.75).abs() < 1e-6);

        let detail = s.get_session(id).unwrap();
        let m = detail.meta.unwrap();
        assert_eq!(m.streams.len(), 1);
        assert_eq!(m.streams[0].recording.as_deref(), Some("2026-08-19_05-57-20_我.wav"));
    }

    #[test]
    fn quality_detects_noise_session() {
        // 模拟用户案例（13:57 会话）：VAD 把环境声音当语音，几乎无停顿（ratio 0.88）
        let meta = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 148000,
                speech_ms: 130000,
                final_segments: 16,
                avg_rms: 0.2,
                max_rms: 0.8,
                non_speech_avg_rms: 0.05,
                recording: None,
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
                ..Default::default()
            }],
            &[
                "我说看嗯行嗯嗯哦不要行嗯".into(),
                "现然做为他规定嗯我还是没有思间别是奖们啊".into(),
                "你三三 d 是吗".into(),
                "嗯你看一下就会会会会有一个案".into(),
            ],
            12345,
            &QualityParams::default(),
        );
        assert_eq!(meta.quality, "noise", "持续有声（ratio 高）应判噪音: {meta:?}");
        assert!(meta.skipped_analysis);

        // 语气词/重复密集 → 文本噪音判 noise（即使 ratio 中等）
        let noisy_text = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 60000,
                speech_ms: 30000,
                final_segments: 5,
                avg_rms: 0.1,
                max_rms: 0.4,
                non_speech_avg_rms: 0.03,
                recording: None,
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
                ..Default::default()
            }],
            &["嗯嗯嗯嗯嗯嗯嗯嗯嗯嗯".into(), "嗯嗯嗯对技术嗯嗯".into()],
            12345,
            &QualityParams::default(),
        );
        assert_eq!(noisy_text.quality, "noise");
        assert!(noisy_text.text_noise > 0.45);

        // 静音会话：几乎无语音
        let silent = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 60000,
                speech_ms: 2000,
                final_segments: 0,
                avg_rms: 0.002,
                max_rms: 0.01,
                non_speech_avg_rms: 0.001,
                recording: None,
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
                ..Default::default()
            }],
            &[],
            12345,
            &QualityParams::default(),
        );
        assert_eq!(silent.quality, "silent");
        assert!(silent.skipped_analysis);
    }

    #[test]
    fn quality_thresholds_are_configurable() {
        // 阈值可配置：默认 0.45 判 noise 的文本，调高阈值后不再判 noise
        let stats = vec![StreamMeta {
            speaker_label: "我".into(),
            total_ms: 60000,
            speech_ms: 36000,
            final_segments: 4,
            avg_rms: 0.1,
            max_rms: 0.4,
            non_speech_avg_rms: 0.03,
            recording: None,
            vad_preset: "standard".into(),
            vad_threshold: 0.5,
            ..Default::default()
        }];
        let texts = &["嗯嗯嗯嗯嗯嗯嗯嗯嗯嗯".into(), "我们确认交期价格".into()];

        let default_params = QualityParams::default();
        let meta = SessionMeta::evaluate(stats.clone(), texts, 1, &default_params);
        assert_eq!(meta.quality, "noise");

        // 调高文本噪音阈值 → 不判 noise（ratio 0.583 → low）
        let relaxed = QualityParams {
            text_noise_threshold: 0.9,
            ..QualityParams::default()
        };
        let stats_low = vec![StreamMeta {
            speaker_label: "我".into(),
            total_ms: 60000,
            speech_ms: 35000,
            final_segments: 4,
            avg_rms: 0.1,
            max_rms: 0.4,
            non_speech_avg_rms: 0.03,
            recording: None,
            vad_preset: "standard".into(),
            vad_threshold: 0.5,
            ..Default::default()
        }];
        let meta2 = SessionMeta::evaluate(stats_low, texts, 1, &relaxed);
        assert_eq!(meta2.quality, "low", "放宽阈值后不应再判噪音: {:?}", meta2.quality);

        // 放宽 max_speech_ratio：持续有声（0.88）不再判 noise
        let lenient = QualityParams {
            max_speech_ratio: 0.95,
            ..QualityParams::default()
        };
        let busy = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 148000,
                speech_ms: 130000,
                final_segments: 16,
                avg_rms: 0.2,
                max_rms: 0.8,
                non_speech_avg_rms: 0.05,
                recording: None,
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
                ..Default::default()
            }],
            &["正常说话内容需要确认".into(), "我们再讨论一下方案".into()],
            1,
            &lenient,
        );
        assert_eq!(busy.quality, "clean", "max_speech_ratio 放宽后持续有声可判正常: {:?}", busy.quality);
    }

    #[test]
    fn auto_detect_background_noise_adjusts_thresholds() {
        // 背景噪音大的环境（非语音块 RMS 高）：auto_detect 提升静音/高能量阈值
        let stats = vec![StreamMeta {
            speaker_label: "我".into(),
            total_ms: 60000,
            speech_ms: 0,
            final_segments: 0,
            avg_rms: 0.02,
            max_rms: 0.1,
            non_speech_avg_rms: 0.02, // 背景噪音水平 0.02
            recording: None,
            vad_preset: "standard".into(),
            vad_threshold: 0.5,
            ..Default::default()
        }];

        // auto_detect=true：silence_rms = 0.02*1.5 = 0.03 > avg_rms 0.02 → 静音
        let meta = SessionMeta::evaluate(stats.clone(), &[], 1, &QualityParams::default());
        assert_eq!(meta.quality, "silent");

        // auto_detect=false + 手工 silence_rms=0.01：avg_rms 0.02 > 0.01 → 有能量无语音 → noise
        let manual = QualityParams {
            auto_detect: false,
            silence_rms: 0.01,
            ..QualityParams::default()
        };
        let meta2 = SessionMeta::evaluate(stats.clone(), &[], 1, &manual);
        assert_eq!(meta2.quality, "noise", "关闭自动检测时用固定阈值判定: {:?}", meta2.quality);
    }
}
