# Scene Language Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decouple audio-stream routing from language/engine selection so single-language scenes use one language for all streams, rename `Translation` → `Bilingual`, add a new `LiveTranslation` scene, and rename the ASR Tab engine selectors from "客户流/我的通道" to "中文引擎/英文引擎".

**Architecture:** Three-layer change: (1) Config layer adds `engine_zh`/`engine_en` to `AsrConfig`, renames `user_language → language` in `SceneParams`, adds `LiveTranslation` scene; (2) Pipeline layer adds `engine_for_language()` helper and scene-aware engine dispatch; (3) UI layer updates types and settings panels. Backward compat via serde aliases so existing TOML files keep working.

**Tech Stack:** Rust (serde, toml), TypeScript, React

---

## File Map

| File | Change |
|------|--------|
| `crates/talksage-config/src/lib.rs` | `AsrConfig`: rename fields; `SceneParams`: rename `user_language→language`, remove per-stream binding; `SceneMode`: add `Bilingual`/`LiveTranslation`; update templates & tests |
| `crates/talksage-pipeline/src/service.rs` | Add `engine_for_language()`, update engine selection in `build_live_config_with`, update `LiveTranslationPolicy` field name |
| `web/src/lib/api.ts` | Update `AppConfig.asr`, `SceneMode`, `SceneParams`, `SessionRuntimeInfo` types |
| `web/src/sections/SettingsSection.tsx` | ASR Tab rename; Scene Tab add language selector, `live_translation` scene, rename `translation→bilingual` |

---

## Task 1: Config layer — `crates/talksage-config/src/lib.rs`

**Files:**
- Modify: `crates/talksage-config/src/lib.rs`

### Changes overview
- `AsrConfig`: `user_engine → engine_zh`, `client_engine → engine_en`
- `SceneMode`: rename `Translation → Bilingual` (serde alias `"translation"` for backward compat), add `LiveTranslation`
- `SceneParams`: rename `user_language → language` (serde alias `"user_language"`), keep `client_language` (dual-purpose: other-party language in Bilingual, target language in LiveTranslation), keep `user_engine`/`client_engine` for Custom mode only
- `scene_params()` templates updated for all modes
- `merge_config()` updated for renamed fields
- Tests updated/added

- [ ] **Step 1: Update `AsrConfig` field names**

In `crates/talksage-config/src/lib.rs`, replace the `AsrConfig` struct and its `Default`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    /// 中文场景使用的引擎（两流均用此引擎）。
    #[serde(alias = "user_engine")]
    pub engine_zh: String,
    /// 英文场景使用的引擎（两流均用此引擎）。
    #[serde(alias = "client_engine")]
    pub engine_en: String,
    /// 推理后端：auto | cpu | cuda | metal。
    pub backend: String,
    /// 专业术语热词和确定性纠错配置。
    pub terminology: TerminologyConfig,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            engine_zh: "paraformer-zh".into(),
            engine_en: "zipformer-en".into(),
            backend: "auto".into(),
            terminology: TerminologyConfig::default(),
        }
    }
}
```

- [ ] **Step 2: Update `SceneMode` — rename Translation → Bilingual, add LiveTranslation**

Replace the `SceneMode` enum, `Default impl`, and `mode_label()`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneMode {
    /// 单人听写：低资源、灵敏 VAD、单流。
    Dictation,
    /// 一对一会话：按输入通道区分双方，两流使用相同语言。
    Conversation,
    /// 双语对话：双方通道使用不同语言，可选双向翻译。
    #[serde(alias = "translation")]   // 兼容旧配置文件
    Bilingual,
    /// 实时翻译：单一语言输入，自动翻译到目标语言输出。
    LiveTranslation,
    /// 多人会议：启用在线声纹聚类和段内换人检测。
    Meeting,
    /// 演讲/课堂：长段单流，开启术语与简报，不运行声纹模型。
    Lecture,
    /// 自定义：使用 `SceneConfig.custom` 全部参数。
    Custom,
}

impl Default for SceneMode {
    fn default() -> Self {
        Self::Conversation
    }
}
```

And update `mode_label()`:

```rust
impl SceneConfig {
    pub fn effective(&self) -> SceneParams {
        match self.mode {
            SceneMode::Custom => self.custom.clone(),
            m => scene_params(m),
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            SceneMode::Dictation => "单人听写",
            SceneMode::Conversation => "一对一会话",
            SceneMode::Bilingual => "双语对话",
            SceneMode::LiveTranslation => "实时翻译",
            SceneMode::Meeting => "多人会议",
            SceneMode::Lecture => "演讲/课堂",
            SceneMode::Custom => "自定义",
        }
    }
}
```

- [ ] **Step 3: Update `SceneParams` — rename `user_language → language`**

Replace the `SceneParams` struct. Keep `user_engine`/`client_engine` for Custom mode backward compat. Add serde alias for `user_language`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneParams {
    pub vad_preset: VadPreset,
    pub vad_threshold: Option<f32>,
    pub vad_min_speech_ms: Option<u64>,
    pub vad_min_silence_ms: Option<u64>,
    pub vad_max_speech_ms: Option<u64>,
    pub denoise_enabled: bool,
    pub denoise_gate: f32,
    pub min_segment_ms: u64,
    /// 自定义模式：用户流使用的引擎（其他模式由 asr.engine_zh/engine_en 决定）。
    pub user_engine: String,
    pub client_enabled: bool,
    /// 自定义模式：客户流使用的引擎（其他模式由 asr.engine_zh/engine_en 决定）。
    pub client_engine: String,
    /// 本场景主语言（所有单语言场景两流均使用此语言）。
    /// 双语场景中为「我的语言」；实时翻译场景中为「输入语言」。
    #[serde(alias = "user_language")]
    pub language: String,
    /// 对方语言：双语场景为「对方讲的语言」；实时翻译场景为「翻译目标语言」。
    pub client_language: String,
    pub translation_mode: TranslationMode,
    pub plugin_allowlist: Vec<String>,
    pub speaker_mode: SpeakerMode,
    pub noise_auto_detect: bool,
}
```

- [ ] **Step 4: Update `SceneParams::default()` and `scene_params()` templates**

Replace `impl Default for SceneParams` and the `scene_params` function:

```rust
impl Default for SceneParams {
    fn default() -> Self {
        scene_params(SceneMode::Conversation)
    }
}

pub fn scene_params(mode: SceneMode) -> SceneParams {
    match mode {
        SceneMode::Dictation => SceneParams {
            vad_preset: VadPreset::Sensitive,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: Some(600),
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 0,
            user_engine: "paraformer-zh".into(),
            client_enabled: false,
            client_engine: "zipformer-en".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: Vec::new(),
            speaker_mode: SpeakerMode::Off,
            noise_auto_detect: true,
        },
        SceneMode::Conversation => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            user_engine: "paraformer-zh".into(),
            client_enabled: true,
            client_engine: "paraformer-zh".into(),
            language: "zh".into(),
            client_language: "zh".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: ["term_explainer", "brief_retriever", "key_point_extractor"]
                .iter().map(|s| s.to_string()).collect(),
            speaker_mode: SpeakerMode::Channel,
            noise_auto_detect: true,
        },
        SceneMode::Bilingual => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            user_engine: "paraformer-zh".into(),
            client_enabled: true,
            client_engine: "zipformer-en".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Bidirectional,
            plugin_allowlist: all_analysis_plugins(),
            speaker_mode: SpeakerMode::Channel,
            noise_auto_detect: true,
        },
        SceneMode::LiveTranslation => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            user_engine: "paraformer-zh".into(),
            client_enabled: false,  // 单流：只有麦克风，翻译用户自己说的话
            client_engine: "zipformer-en".into(),
            language: "zh".into(),
            client_language: "en".into(),  // 翻译目标语言
            translation_mode: TranslationMode::Bidirectional,  // 单流时只有用户段，等效于翻译用户发言
            plugin_allowlist: ["translator"].iter().map(|s| s.to_string()).collect(),
            speaker_mode: SpeakerMode::Off,
            noise_auto_detect: true,
        },
        SceneMode::Meeting => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 0,
            user_engine: "paraformer-zh".into(),
            client_enabled: true,
            client_engine: "paraformer-zh".into(),
            language: "zh".into(),
            client_language: "zh".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: all_analysis_plugins(),
            speaker_mode: SpeakerMode::Voiceprint,
            noise_auto_detect: true,
        },
        SceneMode::Lecture => SceneParams {
            vad_preset: VadPreset::Strict,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: Some(700),
            vad_max_speech_ms: Some(60_000),
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            user_engine: "paraformer-zh".into(),
            client_enabled: false,
            client_engine: "zipformer-en".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: ["term_explainer", "brief_retriever", "key_point_extractor"]
                .iter().map(|s| s.to_string()).collect(),
            speaker_mode: SpeakerMode::Off,
            noise_auto_detect: true,
        },
        SceneMode::Custom => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 0,
            user_engine: "paraformer-zh".into(),
            client_enabled: true,
            client_engine: "zipformer-en".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: all_analysis_plugins(),
            speaker_mode: SpeakerMode::Channel,
            noise_auto_detect: true,
        },
    }
}
```

- [ ] **Step 5: Update `apply_scene_params` — handle renamed field**

In `apply_scene_params`, replace the `user_language` key handler:

```rust
// 旧：if let Some(v) = u.get("user_language")...
// 新：同时接受 "language" 和旧的 "user_language" 键（兼容前端旧版本）
if let Some(v) = u.get("language").or_else(|| u.get("user_language")).and_then(|v| v.as_str()) {
    p.language = v.to_string();
}
```

And keep `client_language` handler unchanged.

- [ ] **Step 6: Update `merge_config` — renamed fields**

In `merge_config`, update the `asr` section:

```rust
asr: AsrConfig {
    engine_zh: take_or(user.asr.engine_zh, default.asr.engine_zh),
    engine_en: take_or(user.asr.engine_en, default.asr.engine_en),
    backend: take_or(user.asr.backend, default.asr.backend),
    terminology: user.asr.terminology,
},
```

And in the `scene.custom` block, replace `user_language`/`client_language` with `language`/`client_language`:

```rust
scene: SceneConfig {
    mode: user.scene.mode,
    custom: SceneParams {
        vad_preset: user.scene.custom.vad_preset,
        vad_threshold: user.scene.custom.vad_threshold.or(default.scene.custom.vad_threshold),
        vad_min_speech_ms: user.scene.custom.vad_min_speech_ms.or(default.scene.custom.vad_min_speech_ms),
        vad_min_silence_ms: user.scene.custom.vad_min_silence_ms.or(default.scene.custom.vad_min_silence_ms),
        vad_max_speech_ms: user.scene.custom.vad_max_speech_ms.or(default.scene.custom.vad_max_speech_ms),
        denoise_enabled: user.scene.custom.denoise_enabled,
        denoise_gate: user.scene.custom.denoise_gate,
        min_segment_ms: user.scene.custom.min_segment_ms,
        user_engine: user.scene.custom.user_engine,
        client_enabled: user.scene.custom.client_enabled,
        client_engine: user.scene.custom.client_engine,
        language: user.scene.custom.language,
        client_language: user.scene.custom.client_language,
        translation_mode: user.scene.custom.translation_mode,
        plugin_allowlist: user.scene.custom.plugin_allowlist,
        speaker_mode: user.scene.custom.speaker_mode,
        noise_auto_detect: user.scene.custom.noise_auto_detect,
    },
},
```

- [ ] **Step 7: Update tests**

Replace the relevant tests in the `#[cfg(test)] mod tests` block. Specifically update:

`scene_conversation_is_the_default` — change `user_language`/`client_language` to `language`:
```rust
#[test]
fn scene_conversation_is_the_default() {
    let cfg = Config::default();
    let p = cfg.scene.effective();
    assert_eq!(cfg.scene.mode, SceneMode::Conversation);
    assert_eq!(p.vad_preset, VadPreset::Standard);
    assert!(!p.denoise_enabled);
    assert_eq!(p.min_segment_ms, 300);
    assert_eq!(p.user_engine, "paraformer-zh");
    assert!(p.client_enabled);
    assert_eq!(p.language, "zh");
    assert_eq!(
        p.plugin_allowlist,
        vec!["term_explainer", "brief_retriever", "key_point_extractor"]
    );
    assert_eq!(p.speaker_mode, SpeakerMode::Channel);
    assert_eq!(p.translation_mode, TranslationMode::Off);
    assert_eq!(p.to_vad_config().effective(), (0.50, 0.25, 0.50, 512, 30.0));
}
```

`scene_templates_express_distinct_workloads` — update for renamed scenes and new scene:
```rust
#[test]
fn scene_templates_express_distinct_workloads() {
    let dictation = scene_params(SceneMode::Dictation);
    let bilingual = scene_params(SceneMode::Bilingual);
    let live_translation = scene_params(SceneMode::LiveTranslation);
    let meeting = scene_params(SceneMode::Meeting);
    let lecture = scene_params(SceneMode::Lecture);

    assert_eq!(dictation.vad_preset, VadPreset::Sensitive);
    assert!(!dictation.client_enabled, "听写场景应单流");
    assert!(dictation.plugin_allowlist.is_empty(), "听写场景应关闭分析插件");
    assert_eq!(dictation.vad_min_silence_ms, Some(600));

    assert_eq!(bilingual.translation_mode, TranslationMode::Bidirectional);
    assert_eq!(bilingual.language, "zh");
    assert_eq!(bilingual.client_language, "en");
    assert!(bilingual.client_enabled);

    assert_eq!(live_translation.translation_mode, TranslationMode::Bidirectional);
    assert!(!live_translation.client_enabled, "实时翻译默认单流");
    assert!(live_translation.plugin_allowlist.contains(&"translator".to_string()));
    assert_eq!(live_translation.language, "zh");
    assert_eq!(live_translation.client_language, "en");

    assert_eq!(meeting.speaker_mode, SpeakerMode::Voiceprint);
    assert_eq!(meeting.language, "zh");

    assert_eq!(lecture.vad_max_speech_ms, Some(60_000));

    let cfg = SceneConfig { mode: SceneMode::Meeting, custom: scene_params(SceneMode::Custom) };
    assert_eq!(cfg.effective().vad_preset, meeting.vad_preset);

    let cfg_custom = SceneConfig { mode: SceneMode::Custom, custom: dictation.clone() };
    assert_eq!(cfg_custom.effective().vad_preset, VadPreset::Sensitive);
}
```

`apply_scene_params_updates_language_translation_and_speaker_policy` — update field name:
```rust
#[test]
fn apply_scene_params_updates_language_translation_and_speaker_policy() {
    let mut p = scene_params(SceneMode::Custom);
    apply_scene_params(
        &mut p,
        &serde_json::json!({
            "language": "en",
            "client_language": "zh",
            "translation_mode": "client_to_user",
            "speaker_mode": "voiceprint"
        }),
    );
    assert_eq!(p.language, "en");
    assert_eq!(p.client_language, "zh");
    assert_eq!(p.translation_mode, TranslationMode::ClientToUser);
    assert_eq!(p.speaker_mode, SpeakerMode::Voiceprint);
}
```

Add new test for backward compat alias:
```rust
#[test]
fn old_user_language_key_still_works_in_apply_scene_params() {
    let mut p = scene_params(SceneMode::Custom);
    apply_scene_params(
        &mut p,
        &serde_json::json!({ "user_language": "en" }),
    );
    assert_eq!(p.language, "en", "旧键 user_language 应映射到 language");
}
```

Add new test for `translation` serde alias:
```rust
#[test]
fn bilingual_mode_deserializes_from_old_translation_key() {
    let _env = env_lock();
    let dir = std::env::temp_dir().join(format!("talksage-cfg-bilingual-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("talksage.toml");
    std::fs::write(&file, "[scene]\nmode = \"translation\"\n").unwrap();
    let cfg = ConfigManager::load(None, Some(&file)).unwrap().snapshot();
    assert_eq!(cfg.scene.mode, SceneMode::Bilingual, "旧 translation 配置应反序列化为 Bilingual");
    std::fs::remove_dir_all(&dir).ok();
}
```

Update the two plugin allowlist tests to use `Bilingual`:
```rust
#[test]
fn bilingual_scene_allows_all_analysis_plugins() {
    let allow = scene_params(SceneMode::Bilingual).plugin_allowlist;
    for id in ["term_explainer", "translator", "brief_retriever", "key_point_extractor"] {
        assert!(allow.contains(&id.to_string()), "双语模式应允许 {id}");
    }
}

#[test]
fn bilingual_scene_enables_translator() {
    let p = scene_params(SceneMode::Bilingual);
    assert!(p.plugin_allowlist.contains(&"translator".to_string()));
    assert_eq!(p.language, "zh");
    assert_eq!(p.client_language, "en");
}
```

Delete or rename `translation_scene_allows_all_analysis_plugins` and `translation_scene_enables_translator` (those were the old tests).

- [ ] **Step 8: Build and test**

```bash
cargo test -p talksage-config 2>&1 | tail -30
```

Expected: all tests pass. If `scene_allowlist` test in talksage-pipeline fails, that's addressed in Task 2.

- [ ] **Step 9: Commit**

```bash
git add crates/talksage-config/src/lib.rs
git commit -m "feat(config): rename Translation→Bilingual, add LiveTranslation, engine_zh/engine_en, language field"
```

---

## Task 2: Pipeline layer — `crates/talksage-pipeline/src/service.rs`

**Files:**
- Modify: `crates/talksage-pipeline/src/service.rs`

- [ ] **Step 1: Add `engine_for_language` helper function**

Add this private function near the top of `service.rs` (before `impl TalkSageService`):

```rust
/// 从语言代码（"zh" / "en"）和全局 ASR 配置解析引擎种类。
/// 中文 → engine_zh，其他一律 engine_en。
fn engine_for_language(lang: &str, asr: &talksage_config::AsrConfig) -> EngineKind {
    if lang == "zh" {
        EngineKind::from_name(&asr.engine_zh).unwrap_or(EngineKind::ParaformerZh)
    } else {
        EngineKind::from_name(&asr.engine_en).unwrap_or(EngineKind::ZipformerEn)
    }
}
```

- [ ] **Step 2: Update engine selection in `build_live_config_with`**

Find and replace the engine resolution block (currently around lines 331–338). Old code:

```rust
let (configured_user_engine, configured_client_engine): (String, String) = match snapshot.scene.mode {
    talksage_config::SceneMode::Custom => (scene.user_engine.clone(), scene.client_engine.clone()),
    _ => (snapshot.asr.user_engine.clone(), scene.client_engine.clone()),
};
let user_engine = req
    .user_engine
    .or_else(|| EngineKind::from_name(&configured_user_engine))
    .unwrap_or(EngineKind::ParaformerZh);
```

New code:

```rust
// 引擎解析规则：
// - Custom 模式：用 scene.user_engine / scene.client_engine（全量用户控制）
// - Bilingual：user 流 = scene.language 对应引擎，client 流 = scene.client_language 对应引擎
// - 其他单语言场景：两流均用 scene.language 对应引擎（消除中英混杂）
let (user_engine_kind, client_engine_kind) = match snapshot.scene.mode {
    talksage_config::SceneMode::Custom => (
        EngineKind::from_name(&scene.user_engine).unwrap_or(EngineKind::ParaformerZh),
        EngineKind::from_name(&scene.client_engine).unwrap_or(EngineKind::ZipformerEn),
    ),
    talksage_config::SceneMode::Bilingual => (
        engine_for_language(&scene.language, &snapshot.asr),
        engine_for_language(&scene.client_language, &snapshot.asr),
    ),
    _ => {
        let e = engine_for_language(&scene.language, &snapshot.asr);
        (e, e)
    }
};
let user_engine = req.user_engine.unwrap_or(user_engine_kind);
```

Then later, replace `client_engine` resolution. Old code:

```rust
let client_engine = EngineKind::from_name(&configured_client_engine).unwrap_or(EngineKind::ZipformerEn);
```

New code:

```rust
let client_engine = client_engine_kind;
```

- [ ] **Step 3: Update `LiveTranslationPolicy` field name**

Find the translation policy construction (currently around lines 427–436). Replace `user_language: scene.user_language.clone()` with `user_language: scene.language.clone()`:

```rust
translation: Some(talksage_plugins::LiveTranslationPolicy {
    mode: match scene.translation_mode {
        TranslationMode::Off => talksage_plugins::LiveTranslationMode::Off,
        TranslationMode::ClientToUser => talksage_plugins::LiveTranslationMode::ClientToUser,
        TranslationMode::Bidirectional => talksage_plugins::LiveTranslationMode::Bidirectional,
    },
    user_language: scene.language.clone(),           // 改：scene.user_language → scene.language
    client_language: scene.client_language.clone(),  // 不变
}),
```

- [ ] **Step 4: Fix any pipeline scene_allowlist test**

If `crates/talksage-pipeline` has a test named `scene_allowlist` or similar that references `SceneMode::Translation`, update it to `SceneMode::Bilingual`:

```bash
grep -rn "Translation\|translation_scene" crates/talksage-pipeline/src/
```

For each hit referencing the old `Translation` scene, update to `Bilingual`. If the test is:

```rust
// old
let p = scene_params(SceneMode::Translation);
assert!(p.plugin_allowlist.contains(...));
```

Change to:

```rust
// new
let p = scene_params(SceneMode::Bilingual);
assert!(p.plugin_allowlist.contains(...));
```

- [ ] **Step 5: Build the pipeline crate**

```bash
cargo build -p talksage-pipeline 2>&1 | tail -30
```

Expected: compiles clean. Fix any remaining references to old field names (`user_engine` on `AsrConfig`, `user_language` on `SceneParams`, `SceneMode::Translation`).

- [ ] **Step 6: Run all Rust tests**

```bash
cargo test --workspace 2>&1 | tail -40
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/talksage-pipeline/src/service.rs
git commit -m "feat(pipeline): scene-aware engine selection via engine_for_language, fix LiveTranslationPolicy field"
```

---

## Task 3: TypeScript types — `web/src/lib/api.ts`

**Files:**
- Modify: `web/src/lib/api.ts`

- [ ] **Step 1: Update `AppConfig.asr`**

Replace the `asr` block in `AppConfig`:

```typescript
asr: {
  engine_zh: string;   // was user_engine
  engine_en: string;   // was client_engine
  backend: string;
  terminology: {
    enabled: boolean;
    hotword_score: number;
    terms: string[];
    corrections: Record<string, string>;
  };
};
```

- [ ] **Step 2: Update `SceneMode` type**

```typescript
export type SceneMode =
  | "dictation"
  | "conversation"
  | "bilingual"         // was "translation"
  | "live_translation"  // new
  | "meeting"
  | "lecture"
  | "custom";
```

- [ ] **Step 3: Update `SceneParams` interface**

```typescript
export interface SceneParams {
  vad_preset: "standard" | "sensitive" | "strict";
  vad_threshold: number | null;
  vad_min_speech_ms: number | null;
  vad_min_silence_ms: number | null;
  vad_max_speech_ms: number | null;
  denoise_enabled: boolean;
  denoise_gate: number;
  min_segment_ms: number;
  /** 自定义模式：用户流引擎（其他模式由 engine_zh/engine_en 决定）。 */
  user_engine: string;
  client_enabled: boolean;
  /** 自定义模式：客户流引擎。 */
  client_engine: string;
  /** 本场景主语言："zh" | "en"。双语中为「我的语言」，实时翻译中为「输入语言」。 */
  language: "zh" | "en";
  /** 对方语言（双语）或翻译目标（实时翻译）。 */
  client_language: "zh" | "en";
  translation_mode: "off" | "client_to_user" | "bidirectional";
  plugin_allowlist: string[];
  speaker_mode: "off" | "channel" | "voiceprint";
  noise_auto_detect: boolean;
}
```

- [ ] **Step 4: Update `SessionRuntimeInfo`**

The `user_engine`/`client_engine` in `SessionRuntimeInfo` come from Rust's session info builder. Keep them as-is (they still represent the actually-used engines, just derived differently now):

```typescript
export interface SessionRuntimeInfo {
  app_version: string;
  scene_mode: string;
  user_engine: string;
  client_engine?: string | null;
  client_enabled: boolean;
  vad_preset: string;
  vad_threshold: number;
  vad_min_silence_ms?: number | null;
  denoise_enabled: boolean;
  min_segment_ms: number;
  input_gain_db: number;
  speaker_mode: string;
  sample_rate: number;
}
```

No change needed here — the session runtime info still reports which engines were actually used.

- [ ] **Step 5: Check for any other usages of old field names**

```bash
grep -rn "user_engine\|client_engine\|\"translation\"" web/src/ --include="*.ts" --include="*.tsx"
```

Note the hits and fix them in Task 4 (SettingsSection.tsx handles most).

- [ ] **Step 6: Commit**

```bash
git add web/src/lib/api.ts
git commit -m "feat(api-types): rename SceneMode translation→bilingual, add live_translation, engine_zh/engine_en"
```

---

## Task 4: Settings UI — `web/src/sections/SettingsSection.tsx`

**Files:**
- Modify: `web/src/sections/SettingsSection.tsx`

This is the largest UI change. All steps modify the same file.

- [ ] **Step 1: Update state initialization — `asr` fields**

Find `initialUserEngine` and `initialClientEngine` helper functions and their state declarations, and update them:

```tsx
// 中文引擎：非自定义场景读全局 asr.engine_zh；自定义读场景参数
const initialEngineZh = () =>
  config?.scene?.mode === "custom"
    ? config?.scene?.custom?.user_engine ?? config?.asr?.engine_zh ?? "paraformer-zh"
    : config?.asr?.engine_zh ?? "paraformer-zh";

const initialEngineEn = () =>
  config?.scene?.mode === "custom"
    ? config?.scene?.custom?.client_engine ?? config?.asr?.engine_en ?? "zipformer-en"
    : config?.asr?.engine_en ?? "zipformer-en";

const [engineEn, setEngineEn] = useState<string>(initialEngineEn);
const [engineZh, setEngineZh] = useState<string>(initialEngineZh);
```

Remove the old `clientEngine`/`userEngine` state variables (or rename them — replacing every occurrence is cleaner).

- [ ] **Step 2: Update `sceneMode` state — rename "translation" → "bilingual"**

In the initial state for `sceneMode`, handle old stored value:

```tsx
const [sceneMode, setSceneMode] = useState<SceneMode>(() => {
  const m = config?.scene?.mode ?? "conversation";
  // 兼容旧配置中的 "translation"（已改名为 "bilingual"）
  return (m === "translation" ? "bilingual" : m) as SceneMode;
});
```

- [ ] **Step 3: Update `sceneCustom` initial state — `user_language → language`**

```tsx
const [sceneCustom, setSceneCustom] = useState<SceneParams>(() => ({
  vad_preset: config?.scene?.custom?.vad_preset ?? "standard",
  vad_threshold: config?.scene?.custom?.vad_threshold ?? null,
  vad_min_speech_ms: config?.scene?.custom?.vad_min_speech_ms ?? null,
  vad_min_silence_ms: config?.scene?.custom?.vad_min_silence_ms ?? null,
  vad_max_speech_ms: config?.scene?.custom?.vad_max_speech_ms ?? null,
  denoise_enabled: config?.scene?.custom?.denoise_enabled ?? false,
  denoise_gate: config?.scene?.custom?.denoise_gate ?? 0.008,
  min_segment_ms: config?.scene?.custom?.min_segment_ms ?? 0,
  user_engine: config?.scene?.custom?.user_engine ?? "paraformer-zh",
  client_enabled: config?.scene?.custom?.client_enabled ?? true,
  client_engine: config?.scene?.custom?.client_engine ?? "zipformer-en",
  language: config?.scene?.custom?.language ?? "zh",       // 新字段（原 user_language）
  client_language: config?.scene?.custom?.client_language ?? "en",
  translation_mode: config?.scene?.custom?.translation_mode ?? "off",
  plugin_allowlist: config?.scene?.custom?.plugin_allowlist ?? [],
  speaker_mode: config?.scene?.custom?.speaker_mode ?? "channel",
  noise_auto_detect: config?.scene?.custom?.noise_auto_detect ?? true,
}));
```

- [ ] **Step 4: Update scene language state for non-custom modes**

Add a separate state for the language selection in template scenes:

```tsx
// 模板场景的语言选择（非自定义模式用，改变后在下次监听时生效）
const [sceneLanguage, setSceneLanguage] = useState<"zh" | "en">(
  config?.scene?.custom?.language ?? "zh"
);
// 双语场景的对方语言
const [sceneClientLanguage, setSceneClientLanguage] = useState<"zh" | "en">(
  config?.scene?.custom?.client_language ?? "en"
);
// 实时翻译场景的目标语言（复用 sceneClientLanguage）
```

- [ ] **Step 5: Update `handleSave` — rename fields in the save payload**

Find the `updates` object in `handleSave` and update the `asr` and `scene` sections:

```tsx
const updates: Record<string, unknown> = {
  llm: { /* unchanged */ },
  plugins: buildPluginUpdates(pluginMeta, pluginValues),
  knowledge_base: { /* unchanged */ },
  asr: {
    engine_zh: engineZh,    // 原 user_engine
    engine_en: engineEn,    // 原 client_engine
    terminology: {
      enabled: terminologyEnabled,
      hotword_score: hotwordScore,
      terms: terminologyTerms.split("\n").map((v) => v.trim()).filter(Boolean),
      corrections,
    },
  },
  audio: { /* unchanged */ },
  recording: { /* unchanged */ },
  quality: { /* unchanged */ },
  webhooks: { /* unchanged */ },
  scene: {
    mode: sceneMode,
    custom: {
      ...sceneCustom,
      // 模板场景的语言选择写入 custom，供自定义场景初始化和「实际生效」参数回溯
      language: sceneMode === "custom" ? sceneCustom.language : sceneLanguage,
      client_language: sceneMode === "custom" ? sceneCustom.client_language : sceneClientLanguage,
    },
  },
};
```

- [ ] **Step 6: Update ASR Tab — rename labels**

Replace the ASR tab JSX (currently around line 614–659). New version:

```tsx
{tab === "asr" && (
  <div>
    <h3 style={groupTitle}>转写引擎</h3>
    <div style={{ display: "flex", gap: 10, marginBottom: 6, flexWrap: "wrap", alignItems: "center" }}>
      <label style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <span style={{ color: "var(--text-2)" }}>中文引擎</span>
        <select
          value={engineZh}
          onChange={(e) => {
            const v = e.target.value;
            setEngineZh(v);
            setSceneCustom((s) => ({ ...s, user_engine: v }));
          }}
          style={inputStyle}
        >
          {modelOptions(engineZh)}
        </select>
      </label>
      <label style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <span style={{ color: "var(--text-2)" }}>英文引擎</span>
        <select
          value={engineEn}
          onChange={(e) => {
            const v = e.target.value;
            setEngineEn(v);
            setSceneCustom((s) => ({ ...s, client_engine: v }));
          }}
          style={inputStyle}
        >
          {modelOptions(engineEn)}
        </select>
      </label>
      <button type="button" onClick={onOpenModels} style={{ fontSize: 12, padding: "4px 10px", cursor: "pointer" }}>
        打开模型管理
      </button>
    </div>
    <div style={hint}>
      中文引擎用于所有中文场景（听写、会话、会议、课堂等）；英文引擎用于英文场景及双语对话的英文通道。
      实时模型持续输出字幕（低延迟）；平衡/准确优先模型在 VAD 段结束后输出（更准确）。
      未安装的引擎请到「模型管理」下载。自定义场景在「场景模式 → 自定义」中单独指定引擎。
    </div>

    <h3 style={{ ...groupTitle, marginTop: 10 }}>麦克风输入电平</h3>
    <label style={labelBlock}>
      输入增益：
      <input
        type="number"
        min={0}
        max={24}
        step={1}
        value={inputGainDb}
        onChange={(e) => setInputGainDb(Math.min(24, Math.max(0, Number(e.target.value) || 0)))}
        style={numStyle}
      /> dB
    </label>
    <div style={hint}>默认 +12dB；无线麦双声道自动选择电平较高的通道并限幅，避免与静音通道平均导致声音变小。</div>
  </div>
)}
```

- [ ] **Step 7: Update Scene Tab — scene buttons and language selector**

Replace the scene mode buttons array in the scene tab. Old `translation` → `bilingual`, add `live_translation`:

```tsx
{tab === "scene" && (
  <div>
    <h3 style={groupTitle}>场景模式</h3>
    <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 8 }}>
      {(
        [
          { key: "dictation", label: "单人听写", desc: "单麦克风、灵敏 VAD、最低资源消耗" },
          { key: "conversation", label: "一对一会话", desc: "双人会话，按输入通道区分双方，两流使用相同语言" },
          { key: "bilingual", label: "双语对话", desc: "双语会话：我说中文，对方说英文（或反向），双向翻译" },
          { key: "live_translation", label: "实时翻译", desc: "说一种语言，自动翻译并显示另一种语言" },
          { key: "meeting", label: "多人会议", desc: "两人以上，启用 WeSpeaker 在线角色识别" },
          { key: "lecture", label: "演讲/课堂", desc: "长段单流，术语和简报增强，关闭角色识别" },
          { key: "custom", label: "自定义", desc: "使用下方全部参数" },
        ] as const
      ).map((m) => (
        <button
          key={m.key}
          onClick={() => setSceneMode(m.key)}
          title={m.desc}
          style={{
            padding: "6px 14px",
            borderRadius: 8,
            border: "1px solid var(--border)",
            cursor: "pointer",
            font: "inherit",
            fontSize: 12,
            fontWeight: 600,
            background: sceneMode === m.key ? "var(--me)" : "var(--surface-2)",
            color: sceneMode === m.key ? "#fff" : "var(--text-2)",
          }}
        >
          {m.label}
        </button>
      ))}
    </div>
```

- [ ] **Step 8: Add language selectors for template scenes**

After the scene buttons, add language selector for non-custom scenes, then the readonly summary or custom form:

```tsx
    {/* 单语言场景：显示语言选择器 */}
    {(sceneMode === "dictation" || sceneMode === "conversation" || sceneMode === "meeting" || sceneMode === "lecture") && (
      <div style={{ marginBottom: 8 }}>
        <label>
          识别语言：
          <select
            value={sceneLanguage}
            onChange={(e) => setSceneLanguage(e.target.value as "zh" | "en")}
            style={{ ...inputStyle, marginLeft: 8 }}
          >
            <option value="zh">中文</option>
            <option value="en">英语</option>
          </select>
        </label>
        <span style={{ ...hint, display: "inline", marginLeft: 12 }}>
          两条通道均使用此语言识别（按通道区分说话人，不混合）
        </span>
      </div>
    )}

    {/* 双语对话：我的语言 + 对方语言 */}
    {sceneMode === "bilingual" && (
      <div style={{ display: "flex", gap: 12, marginBottom: 8, flexWrap: "wrap" }}>
        <label>
          我的语言：
          <select
            value={sceneLanguage}
            onChange={(e) => setSceneLanguage(e.target.value as "zh" | "en")}
            style={{ ...inputStyle, marginLeft: 8 }}
          >
            <option value="zh">中文</option>
            <option value="en">英语</option>
          </select>
        </label>
        <label>
          对方语言：
          <select
            value={sceneClientLanguage}
            onChange={(e) => setSceneClientLanguage(e.target.value as "zh" | "en")}
            style={{ ...inputStyle, marginLeft: 8 }}
          >
            <option value="zh">中文</option>
            <option value="en">英语</option>
          </select>
        </label>
      </div>
    )}

    {/* 实时翻译：输入语言 + 翻译目标语言 */}
    {sceneMode === "live_translation" && (
      <div style={{ display: "flex", gap: 12, marginBottom: 8, flexWrap: "wrap" }}>
        <label>
          我说的语言：
          <select
            value={sceneLanguage}
            onChange={(e) => setSceneLanguage(e.target.value as "zh" | "en")}
            style={{ ...inputStyle, marginLeft: 8 }}
          >
            <option value="zh">中文</option>
            <option value="en">英语</option>
          </select>
        </label>
        <label>
          翻译为：
          <select
            value={sceneClientLanguage}
            onChange={(e) => setSceneClientLanguage(e.target.value as "zh" | "en")}
            style={{ ...inputStyle, marginLeft: 8 }}
          >
            <option value="zh">中文</option>
            <option value="en">英语</option>
          </select>
        </label>
      </div>
    )}
```

- [ ] **Step 9: Update readonly scene summaries**

Update the readonly summary block for `bilingual` and add `live_translation`. Replace the relevant conditionals:

```tsx
    {sceneMode !== "custom" ? (
      <div style={{ background: "var(--surface-2)", borderRadius: 6, padding: 8, fontSize: 11, color: "var(--text-2)", lineHeight: 1.9 }}>
        {sceneMode === "dictation" && (
          <>
            <div>· 单麦克风听写；灵敏 VAD，短句不丢</div>
            <div>· 角色、翻译和分析插件关闭，资源消耗最低</div>
          </>
        )}
        {sceneMode === "conversation" && (
          <>
            <div>· 双方使用相同语言实时识别；最短提交 300ms</div>
            <div>· 按麦克风/系统音频通道标记"我/对方"，不加载声纹模型</div>
            <div>· 开启术语和简报，关闭翻译</div>
          </>
        )}
        {sceneMode === "bilingual" && (
          <>
            <div>· 两通道分别使用各自语言的识别模型，不混用</div>
            <div>· 按通道确定角色，双向实时翻译；不加载声纹模型</div>
            <div>· macOS 远程通话需选择可捕获系统音频的设备</div>
          </>
        )}
        {sceneMode === "live_translation" && (
          <>
            <div>· 单流（麦克风），用你选择的语言说话</div>
            <div>· 实时转写并翻译成目标语言，同时在界面显示原文和译文</div>
            <div>· 适合实时演示、口译练习或字幕辅助场景</div>
          </>
        )}
        {sceneMode === "meeting" && (
          <>
            <div>· 两人以上会议，开启 WeSpeaker 在线聚类和段内换人检测</div>
            <div>· 术语、简报等会议分析开启，翻译默认关闭</div>
            <div>· 角色识别会增加 CPU 和内存占用</div>
          </>
        )}
        {sceneMode === "lecture" && (
          <>
            <div>· 单流，严格 VAD；段尾静音 700ms，最长语音 60s</div>
            <div>· 关闭角色识别；开启术语和简报，适合长时间连续发言</div>
          </>
        )}
        <div style={{ marginTop: 6, color: "var(--muted)" }}>
          内置模板只读；选择「自定义」可修改全部参数。参数在下次开始监听时生效。
        </div>
      </div>
    ) : (
      /* 自定义场景表单 — 保持不变 */
      ...
    )}
  </div>
)}
```

- [ ] **Step 10: Update custom scene form — `user_language → language`**

Inside the custom scene form, find and replace language selectors. Old:

```tsx
<label style={labelBlock}>
  我的语言：
  <select value={sceneCustom.user_language} onChange={(e) => setSceneCustom({ ...sceneCustom, user_language: e.target.value as ... })} ...>
```

New:

```tsx
<label style={labelBlock}>
  我的语言：
  <select
    value={sceneCustom.language}
    onChange={(e) => setSceneCustom({ ...sceneCustom, language: e.target.value as "zh" | "en" })}
    style={inputStyle}
  >
    <option value="zh">中文</option>
    <option value="en">英语</option>
  </select>
</label>
```

- [ ] **Step 11: TypeScript compile check**

```bash
cd web && npx tsc --noEmit 2>&1 | head -40
```

Fix any type errors (most likely remaining `user_language` references or `"translation"` in SceneMode).

- [ ] **Step 12: Commit**

```bash
git add web/src/sections/SettingsSection.tsx web/src/lib/api.ts
git commit -m "feat(ui): rename translation→bilingual scene, add live_translation, engine_zh/engine_en ASR tab"
```

---

## Self-Review

### Spec coverage check

| Requirement | Covered by |
|-------------|-----------|
| 中英混杂根因：非自定义场景两流应用同一语言 | Task 2 Step 2: `engine_for_language`, single-lang scenes both streams use `scene.language` |
| 单语言场景加语言选择器 | Task 4 Step 8: language selector for dictation/conversation/meeting/lecture |
| 新增"实时翻译"场景 | Task 1 Step 2: `LiveTranslation` in `SceneMode`; Task 1 Step 4: template; Task 4 Steps 8-9: UI |
| "双语对话"替代旧"translation" | Task 1 Step 2: `Bilingual` with serde alias; Task 4 Step 7: button rename |
| ASR 转写 Tab "客户流/我的通道" → "中文引擎/英文引擎" | Task 4 Step 6 |
| 旧配置 `"translation"` 模式向后兼容 | Task 1 Step 2: `#[serde(alias = "translation")]`; Task 4 Step 2: JS alias in state init |
| 旧配置 `user_language` 字段向后兼容 | Task 1 Step 3: `#[serde(alias = "user_language")]`; Task 1 Step 5: `apply_scene_params` accepts both keys |

### Placeholder scan
No "TBD", "TODO", or "implement later" in this plan.

### Type consistency
- `SceneParams.language` used consistently in all steps (Tasks 1–4).
- `AsrConfig.engine_zh`/`engine_en` consistently named across Tasks 1–3.
- `SceneMode::Bilingual` / `"bilingual"` consistent across Rust and TypeScript.
- `LiveTranslationPolicy.user_language` (in talksage-plugins, unchanged) receives `scene.language` from service.rs Task 2 Step 3 — the field name in plugins crate stays `user_language`; only the *source* changes.
