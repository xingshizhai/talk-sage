//! finalizer 的宿主实现：把 `TalkSageService::finish()` 里原本内联的会后逻辑
//! （质量评估、webhook 推送）实现成插件层定义的 `QualityDeps` / `WebhookDeps`。
//!
//! 为什么住在 pipeline 侧：talksage-plugins 不依赖 talksage-session /
//! talksage-config，也不该依赖 —— 插件层定义「需要什么能力」，宿主提供
//! 「怎么做到」。依赖方向反过来的话，插件层会被拖进持久化与配置细节。
//!
//! 依赖按会话构造，在 `build_registry` 之前 —— 钩子一旦进了 `HookRegistry`
//! 就是不可变的 `Arc<dyn ...>`，`register` 是唯一的注入时机。

use std::sync::{Arc, Mutex};

use talksage_config::ConfigManager;
use talksage_plugins::{QualityDeps, WebhookDeps};
use talksage_session::{SessionStore, StreamMeta};

use crate::service::unix_secs;

/// `session_quality` finalizer 的宿主实现（原先是 `finish()` 里的内联块）。
///
/// 按会话构造，不能挂在 `TalkSageService` 上：流统计与段文本只活在本次监听的
/// 内存里（落库的只有段本身），换一场会就是另一份数据。
pub(crate) struct QualityHost {
    pub config: Arc<ConfigManager>,
    pub store: Arc<SessionStore>,
    pub stats: Arc<Mutex<Vec<StreamMeta>>>,
    pub texts: Arc<Mutex<Vec<String>>>,
    pub master_recording: Arc<Mutex<Option<String>>>,
}

impl QualityDeps for QualityHost {
    fn evaluate_and_store(&self, session_id: i64) -> anyhow::Result<Option<String>> {
        let stats = self.stats.lock().unwrap().clone();
        if stats.is_empty() {
            return Ok(None); // 一条流统计都没有：没什么可评的，不算失败
        }
        let texts = self.texts.lock().unwrap().clone();
        let snapshot = self.config.snapshot();
        let mut params = talksage_session::QualityParams::from_config(&snapshot.quality);
        params.auto_detect = snapshot.scene.effective().noise_auto_detect;
        let mut meta = talksage_session::SessionMeta::evaluate(stats, &texts, unix_secs(), &params);
        meta.master_recording = self.master_recording.lock().unwrap().clone();
        // 记录本次运行的模型/场景/主要参数：事后可对比不同 ASR 配置的质量
        // 差异，或按相同参数重放历史录音（见 SessionRuntimeInfo）。
        meta.runtime_info = Some(runtime_info_from_config(&snapshot));
        if let Err(e) = self.store.set_session_meta(session_id, &meta) {
            // 写不进 meta 不该拖垮整条链：会话本身已经落库，报个警继续。
            log::warn!("保存会话元数据失败: {e}");
        }
        log::info!(
            "会话 #{session_id} 质量详情：时长 {}s，语音占比 {:.0}%，文本噪音 {:.2}，跳过下游分析={}",
            meta.duration_ms / 1000,
            meta.speech_ratio * 100.0,
            meta.text_noise,
            meta.skipped_analysis,
        );
        Ok(Some(meta.quality_label().to_string()))
    }
}

/// 从配置快照构建运行环境信息（模型/场景/主要参数）。
fn runtime_info_from_config(cfg: &talksage_config::Config) -> talksage_session::SessionRuntimeInfo {
    let scene = cfg.scene.effective();
    let vad = scene.to_vad_config().effective();
    let audio = &cfg.audio;
    let asr = &cfg.asr;
    // 与 service.rs engine_for_language 逻辑保持一致：lang="zh" → engine_zh，其他 → engine_en。
    let engine_for_lang = |lang: &str| -> String {
        if lang == "zh" { asr.engine_zh.clone() } else { asr.engine_en.clone() }
    };
    let user_engine = match cfg.scene.mode {
        talksage_config::SceneMode::Custom => {
            if scene.user_engine.is_empty() { engine_for_lang(&scene.language) } else { scene.user_engine.clone() }
        }
        _ => engine_for_lang(&scene.language),
    };
    let client_engine_name = match cfg.scene.mode {
        talksage_config::SceneMode::Custom => {
            if scene.client_engine.is_empty() { engine_for_lang(&scene.client_language) } else { scene.client_engine.clone() }
        }
        talksage_config::SceneMode::Bilingual => engine_for_lang(&scene.client_language),
        _ => engine_for_lang(&scene.language),
    };
    talksage_session::SessionRuntimeInfo {
        app_version: talksage_core::VERSION.to_string(),
        scene_mode: format!("{:?}", cfg.scene.mode).to_ascii_lowercase(),
        user_engine,
        client_engine: scene.client_enabled.then_some(client_engine_name),
        client_enabled: scene.client_enabled,
        vad_preset: format!("{:?}", scene.vad_preset).to_ascii_lowercase(),
        vad_threshold: vad.0,
        vad_min_silence_ms: scene.vad_min_silence_ms,
        denoise_enabled: scene.denoise_enabled,
        min_segment_ms: scene.min_segment_ms,
        input_gain_db: audio.input_gain_db,
        speaker_mode: format!("{:?}", scene.speaker_mode).to_ascii_lowercase(),
        sample_rate: talksage_audio::TARGET_SAMPLE_RATE,
    }
}

/// `webhook` finalizer 的宿主实现（原先是 `finish()` 里的内联块）。
pub(crate) struct WebhookHost {
    pub config: Arc<ConfigManager>,
    pub store: Arc<SessionStore>,
}

impl WebhookDeps for WebhookHost {
    /// 载荷是从库里现取的会话详情，其中的 meta 已由链上游的 `session_quality`
    /// 写好 —— 这就是两者顺序不能反的原因。
    ///
    /// **返回 `Ok(())` 只表示已派发，不表示已送达**：真正的推送在下面的独立
    /// 线程里，它的成败进不了 `FinalizeReport`。别把「无失败项」读成「都推成功了」。
    fn push(&self, session_id: i64) -> anyhow::Result<()> {
        // 第二道闸：配置在会后现取，会话进行中改了开关也算数（与搬迁前一致）。
        // 插件自身的 enabled 只决定 finalizer 装不装，看不到 [webhooks]。
        let snap = self.config.snapshot();
        let wh_cfg = snap.webhooks.clone();
        if !wh_cfg.enabled || wh_cfg.urls.is_empty() {
            return Ok(());
        }
        let proxy = snap.network.proxy_url().map(str::to_string);
        let store = self.store.clone();
        // 独立线程：webhook 是网络 IO，不能拖住会话收尾（停止监听后 UI 要立刻可用）。
        // 代价是线程内的失败进不了 FinalizeReport —— 与搬迁前一样，只能靠日志。
        std::thread::spawn(move || {
            if let Ok(detail) = store.get_session(session_id) {
                let results = talksage_session::trigger_meeting_webhooks(&detail, &wh_cfg, proxy.as_deref());
                for r in &results {
                    log::info!(
                        "webhook {}: {}（{}）",
                        if r.ok { "成功" } else { "失败" },
                        r.url,
                        r.message
                    );
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_config::Config;

    struct Fixture {
        dir: std::path::PathBuf,
        config: Arc<ConfigManager>,
        store: Arc<SessionStore>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(cfg: Config) -> Fixture {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "talksage-finalize-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Arc::new(ConfigManager::from_config(cfg, dir.clone()));
        let store = Arc::new(
            SessionStore::open(&dir.join("sessions.db").to_string_lossy()).unwrap(),
        );
        Fixture { dir, config, store }
    }

    fn stream(total_ms: u64, speech_ms: u64) -> StreamMeta {
        StreamMeta {
            speaker_label: "我".into(),
            total_ms,
            speech_ms,
            final_segments: 3,
            avg_rms: 0.05,
            max_rms: 0.3,
            non_speech_avg_rms: 0.01,
            ..Default::default()
        }
    }

    fn quality_host(f: &Fixture, stats: Vec<StreamMeta>, texts: Vec<String>) -> QualityHost {
        QualityHost {
            config: f.config.clone(),
            store: f.store.clone(),
            stats: Arc::new(Mutex::new(stats)),
            texts: Arc::new(Mutex::new(texts)),
            master_recording: Arc::new(Mutex::new(None)),
        }
    }

    /// 搬迁前的 `if !stats.is_empty()` 守卫：没有流统计就什么都不做，
    /// 而且不能算失败（否则每场没跑起来的会都会进 FinalizeReport.failed）。
    #[test]
    fn no_stream_stats_means_skip_not_failure() {
        let f = fixture(Config::default());
        let sid = f.store.start_session(1_000).unwrap();
        let host = quality_host(&f, Vec::new(), Vec::new());
        assert!(host.evaluate_and_store(sid).unwrap().is_none());
        assert!(f.store.get_session(sid).unwrap().meta.is_none(), "不该写 meta");
    }

    #[test]
    fn evaluating_writes_meta_and_returns_the_label() {
        let f = fixture(Config::default());
        let sid = f.store.start_session(1_000).unwrap();
        let host = quality_host(&f, vec![stream(60_000, 45_000)], vec!["今天我们聊一下方案".into()]);
        *host.master_recording.lock().unwrap() = Some("session-1_master.wav".into());
        let label = host.evaluate_and_store(sid).unwrap().expect("有统计就该有结论");
        let meta = f.store.get_session(sid).unwrap().meta.expect("meta 应已落库");
        assert_eq!(meta.quality_label(), label, "返回的标签应与落库的一致");
        assert_eq!(meta.master_recording.as_deref(), Some("session-1_master.wav"));
        // 运行环境快照应写入（默认场景=一对一会话，用户引擎=qwen3-asr）
        let ri = meta.runtime_info.expect("应写入运行环境快照");
        assert_eq!(ri.scene_mode, "conversation");
        assert_eq!(ri.user_engine, "qwen3-asr");
        assert!(ri.client_enabled, "默认场景应双流");
        assert_eq!(ri.sample_rate, 16_000);
        assert_eq!(ri.app_version, talksage_core::VERSION);
    }

    /// 第二道闸：`[webhooks]` 关闭时不推送。插件自身的 enabled 管不到这里 ——
    /// 它在装载期就定了，看不到会后的配置。
    #[test]
    fn webhook_push_is_a_no_op_when_the_config_is_off() {
        let f = fixture(Config::default()); // webhooks 默认 enabled=false
        assert!(!f.config.snapshot().webhooks.enabled);
        let host = WebhookHost { config: f.config.clone(), store: f.store.clone() };
        // 没有可达 URL，若真的推送这里会挂网络；返回 Ok 即说明在闸口就返回了
        assert!(host.push(1).is_ok());
    }

    /// enabled 为真但 urls 为空同样不推 —— 两个条件是与的关系，搬迁前后一致。
    #[test]
    fn webhook_push_is_a_no_op_when_enabled_but_no_urls() {
        let mut cfg = Config::default();
        cfg.webhooks.enabled = true;
        cfg.webhooks.urls = Vec::new();
        let f = fixture(cfg);
        let host = WebhookHost { config: f.config.clone(), store: f.store.clone() };
        assert!(host.push(1).is_ok());
    }
}
