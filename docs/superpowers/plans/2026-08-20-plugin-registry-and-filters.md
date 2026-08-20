# 插件注册表与 filter 链实施计划（阶段 1–2）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建立插件注册表与三类钩子的骨架，把 `short_segment` 与 `cross_stream_dedup` 两个硬编码功能搬成 `EventFilter`，并把现有 `AnalyzerPlugin` 改名接入为 `SegmentObserver`。

**Architecture:** 一个 `Plugin` trait 管身份与默认配置，插件在 `register()` 里把自己挂进 `HookRegistry`。filter 链放在事件产生点（`StreamWorker` 里 `emit` 与 `on_final` 之前），被吞掉的事件既不进 sink 也不触发 observer。注册机制是显式中心表 `builtin_plugins()`，列表顺序即执行顺序。

**Tech Stack:** Rust 2021、`serde_json`（插件配置载体）、现有 `talksage-plugins` / `talksage-pipeline` crate，不新增依赖、不新增 crate。

**对应设计：** [2026-08-20-everything-is-a-plugin-design.md](../specs/2026-08-20-everything-is-a-plugin-design.md) 阶段 1–2。阶段 3–5（observer 迁移、finalizer 迁移、配置改造）待本计划完成、接缝验证后另行成文。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| 创建 `crates/talksage-plugins/src/registry.rs` | `Plugin` / `EventFilter` / `SegmentObserver` trait、`PluginConfig`、`HookRegistry`、`build_registry()` |
| 创建 `crates/talksage-plugins/src/short_segment.rs` | `short_segment` 插件：final 段时长低于阈值则吞掉 |
| 创建 `crates/talksage-plugins/src/cross_stream_dedup.rs` | `cross_stream_dedup` 插件：跨流回声重复段吞掉 |
| 创建 `crates/talksage-plugins/src/builtin.rs` | `builtin_plugins()` 中心表，列表顺序即钩子顺序 |
| 修改 `crates/talksage-plugins/src/lib.rs` | 挂载新模块；`AnalyzerPlugin` 改名 `SegmentObserver` 并迁入 `registry.rs` |
| 修改 `crates/talksage-pipeline/src/lib.rs` | `LivePipelineConfig` 持有 `HookRegistry`；`StreamWorker` 应用 filter 链；删除 `min_commit_ms` 提前 return 与 emit 包装内的去重 |
| 修改 `crates/talksage-pipeline/src/service.rs` | 用 `build_registry()` 替代硬编码的插件 if 链 |
| 创建 `crates/talksage-pipeline/tests/characterization.rs` | 特征化测试：固定语料的事件序列对比 golden 文件 |
| 创建 `crates/talksage-pipeline/tests/golden/zh_single_stream.txt` | golden 快照 |

---

## Task 1: 特征化测试（搬家前的安全网）

**Files:**
- Create: `crates/talksage-pipeline/tests/characterization.rs`
- Create: `crates/talksage-pipeline/tests/golden/zh_single_stream.txt`（由测试首次运行生成）

- [ ] **Step 1: 写特征化测试**

创建 `crates/talksage-pipeline/tests/characterization.rs`：

```rust
//! 特征化测试：把固定语料跑出的事件序列归一化后与 golden 文件比对。
//!
//! 目的是在插件化重构期间锁住行为 —— 它不判断行为「对不对」，只判断
//! 「和重构前一不一样」。预期内的行为变更需显式更新 golden 文件并在
//! 提交信息里说明。
//!
//! 重新生成：TALKSAGE_UPDATE_GOLDEN=1 cargo test -p talksage-pipeline --test characterization

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use talksage_asr::EngineKind;
use talksage_core::DomainEvent;
use talksage_pipeline::{AudioInput, LivePipelineConfig, SessionRuntime, StreamConfig};

fn skip(reason: &str) {
    let require = matches!(
        std::env::var("TALKSAGE_REQUIRE_MODELS").ok().as_deref(),
        Some("1") | Some("true")
    );
    assert!(
        !require,
        "集成测试资源缺失（TALKSAGE_REQUIRE_MODELS=1 要求必须真实运行）: {reason}"
    );
    eprintln!("跳过：{reason}");
}

fn model_root() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    for cand in [here.join("../../models"), PathBuf::from("models")] {
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

/// 事件归一化：只保留对行为有意义的字段。
///
/// 刻意丢弃 ts_ms / rms / revision —— 它们随采样与实现细节浮动，
/// 纳入 golden 会让测试因无关变更而红。
fn normalize(evs: &[DomainEvent]) -> String {
    let mut out = String::new();
    for ev in evs {
        match ev {
            DomainEvent::Segment { speaker_label, text, is_partial: false, duration_ms, .. } => {
                out.push_str(&format!("final\t{speaker_label}\t{duration_ms}\t{text}\n"));
            }
            DomainEvent::Segment { is_partial: true, .. } => {
                // partial 数量随线程调度浮动，只记类型不记内容
                out.push_str("partial\n");
            }
            DomainEvent::Status { stage, .. } => out.push_str(&format!("status\t{stage:?}\n")),
            DomainEvent::Term { status, .. } => out.push_str(&format!("term\t{status:?}\n")),
            DomainEvent::Translation { status, direction, .. } => {
                out.push_str(&format!("translation\t{status:?}\t{direction:?}\n"))
            }
            DomainEvent::Metrics { .. } => out.push_str("metrics\n"),
            DomainEvent::Nudge { .. } => out.push_str("nudge\n"),
            DomainEvent::SessionStats { speaker_label, final_segments, .. } => {
                out.push_str(&format!("stats\t{speaker_label}\t{final_segments}\n"))
            }
            DomainEvent::Level { .. } => {} // 高频且随机，完全忽略
            other => out.push_str(&format!("{}\n", other_kind(other))),
        }
    }
    out
}

fn other_kind(ev: &DomainEvent) -> &'static str {
    match ev {
        DomainEvent::Brief { .. } => "brief",
        DomainEvent::State { .. } => "state",
        DomainEvent::KeyPoint { .. } => "keypoint",
        DomainEvent::Snapshot { .. } => "snapshot",
        _ => "other",
    }
}

fn run_and_collect(cfg: LivePipelineConfig) -> Vec<DomainEvent> {
    let events: Arc<Mutex<Vec<DomainEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let sink: Arc<dyn Fn(DomainEvent) + Send + Sync> =
        Arc::new(move |ev: DomainEvent| sink_events.lock().unwrap().push(ev));

    let mut runtime = SessionRuntime::new(cfg);
    runtime.start(sink).expect("pipeline 启动失败");
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let done = {
            let evs = events.lock().unwrap();
            evs.iter().any(|e| {
                matches!(e, DomainEvent::Status { stage: talksage_core::StatusStage::Idle, .. })
            })
        };
        if done || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(runtime.stop_with_timeout(Duration::from_secs(5)), "管道应在时限内结束");
    let result = events.lock().unwrap().clone();
    result
}

fn zh_pipeline(root: &Path, wav: &Path, min_commit_ms: u64) -> LivePipelineConfig {
    LivePipelineConfig {
        vad_model: root.join("silero-vad").join("silero_vad.onnx"),
        chunk_ms: 100,
        vad: talksage_config::VadConfig::default(),
        denoise: talksage_config::DenoiseConfig::default(),
        asr_threads: 2,
        user: StreamConfig {
            engine_kind: EngineKind::ParaformerZh,
            model_dir: root.join("sherpa-onnx-streaming-paraformer-zh"),
            input: AudioInput::File(wav.to_path_buf()),
            speaker_id: 0,
            speaker_label: "我".into(),
        },
        client: None,
        plugins: Vec::new(),
        plugin_ctx: talksage_plugins::PluginContext::new(),
        recording_dir: None,
        runtime: Arc::new(talksage_pipeline::RuntimeParams::default()),
        speaker: None,
        engine_pool: None,
        min_commit_ms,
    }
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name)
}

/// 与 golden 比对；设 TALKSAGE_UPDATE_GOLDEN=1 时改为写入。
fn assert_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    if std::env::var("TALKSAGE_UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        eprintln!("已更新 golden: {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "缺少 golden 文件 {}；首次生成请运行：\n  \
             TALKSAGE_UPDATE_GOLDEN=1 cargo test -p talksage-pipeline --test characterization",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "事件序列与 golden 不一致。若为预期内的行为变更，用 TALKSAGE_UPDATE_GOLDEN=1 更新并在提交信息里说明原因。"
    );
}

#[test]
fn zh_single_stream_event_sequence_is_stable() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = root.join("sherpa-onnx-streaming-paraformer-zh").join("0.wav");
    if !wav.is_file() || !root.join("silero-vad").join("silero_vad.onnx").is_file() {
        return skip("模型/VAD/测试音频不完整");
    }
    let evs = run_and_collect(zh_pipeline(&root, &wav, 0));
    assert_golden("zh_single_stream.txt", &normalize(&evs));
}
```

- [ ] **Step 2: 首次生成 golden 并确认内容合理**

```bash
cd /Users/robot/projects/talk-sage
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export SHERPA_ONNX_ARCHIVE_DIR="$PWD/.tools/sherpa-onnx-archives"
export TALKSAGE_MODELS_DIR="$PWD/models"
TALKSAGE_UPDATE_GOLDEN=1 cargo test -p talksage-pipeline --test characterization -- --nocapture
cat crates/talksage-pipeline/tests/golden/zh_single_stream.txt
```

预期：打印「已更新 golden」，文件内含若干 `status` 行、`partial` 行、以 `final\t我\t<时长>\t<文本>` 形式的最终段，以及末尾 `stats` 行。**人工确认 final 行数与文本非空** —— golden 记录的是既有行为，如果这一步就是空的，说明测试没真正跑起来，不要继续。

- [ ] **Step 3: 再跑一次，确认比对通过（幂等）**

```bash
cargo test -p talksage-pipeline --test characterization
```

预期：`test result: ok. 1 passed`。若不稳定（两次结果不同），说明归一化还漏了浮动字段，先修 `normalize()` 再往下走。

- [ ] **Step 4: 提交**

```bash
git add crates/talksage-pipeline/tests/characterization.rs crates/talksage-pipeline/tests/golden/
git commit -m "test(pipeline): characterization golden for the plugin refactor

插件化重构要搬动 8 个功能，任一语义漂移都是回归。这个测试把固定语料
的事件序列归一化后锁进 golden 文件，只回答「和重构前一不一样」。
ts_ms / rms / revision 等浮动字段刻意排除在外。"
```

---

## Task 2: `PluginConfig` 与配置合并

**Files:**
- Create: `crates/talksage-plugins/src/registry.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`
- Modify: `crates/talksage-plugins/Cargo.toml`

- [ ] **Step 1: 加依赖**

`crates/talksage-plugins/Cargo.toml` 当前 `[dependencies]` 只有 `talksage-core` / `talksage-knowledge` / `talksage-llm` / `anyhow`。整段替换为：

```toml
[dependencies]
talksage-core = { path = "../talksage-core" }
talksage-knowledge = { path = "../talksage-knowledge" }
talksage-llm = { path = "../talksage-llm" }
anyhow = { workspace = true }
serde_json = { workspace = true }
log = "0.4"
```

`serde_json` 是 `PluginConfig` 的载体，`log` 供 Task 5/6/7 的日志使用。两者都是本计划新增，后续任务不再重复添加。

- [ ] **Step 2: 写失败的测试**

创建 `crates/talksage-plugins/src/registry.rs`，先只写测试：

```rust
//! 插件注册表：Plugin trait、三类钩子、配置载体。

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_values_override_defaults_and_unknown_keys_are_kept() {
        let mut cfg = PluginConfig::from_value(json!({"enabled": true, "cooldown_seconds": 30.0}));
        cfg.merge(&json!({"cooldown_seconds": 5.0}));
        assert_eq!(cfg.get_f64("cooldown_seconds", 0.0), 5.0);
        assert!(cfg.enabled(), "未覆盖的 enabled 应保留默认值");
    }

    #[test]
    fn missing_keys_fall_back_to_the_supplied_default() {
        let cfg = PluginConfig::from_value(json!({}));
        assert_eq!(cfg.get_u64("min_ms", 300), 300);
        assert_eq!(cfg.get_f64("ratio", 0.5), 0.5);
        assert!(cfg.get_bool("whatever", true));
    }

    #[test]
    fn enabled_defaults_to_true_and_can_be_switched_off() {
        assert!(PluginConfig::from_value(json!({})).enabled());
        assert!(!PluginConfig::from_value(json!({"enabled": false})).enabled());
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

先在 `crates/talksage-plugins/src/lib.rs` 顶部加 `pub mod registry;`，然后：

```bash
cargo test -p talksage-plugins --lib registry 2>&1 | head -20
```

预期：`error[E0433]: failed to resolve: use of undeclared type PluginConfig`（编译失败，因为类型还不存在）。

- [ ] **Step 4: 实现 PluginConfig**

把以下内容插入 `registry.rs` 顶部（在 `#[cfg(test)]` 之前）：

```rust
use serde_json::Value;

/// 插件配置载体。用 serde_json::Value 与 ConfigManager 已有的
/// apply_scene_params(p, u: &Value) 模式保持一致，不引入新的 schema 机制。
#[derive(Debug, Clone)]
pub struct PluginConfig(Value);

impl Default for PluginConfig {
    fn default() -> Self {
        Self(Value::Object(Default::default()))
    }
}

impl PluginConfig {
    pub fn from_value(v: Value) -> Self {
        Self(v)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// 用户值覆盖：只覆盖 user 里出现的键，其余保留默认。
    pub fn merge(&mut self, user: &Value) {
        let (Value::Object(base), Value::Object(over)) = (&mut self.0, user) else {
            return;
        };
        for (k, v) in over {
            base.insert(k.clone(), v.clone());
        }
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        self.0.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    pub fn get_u64(&self, key: &str, default: u64) -> u64 {
        self.0.get(key).and_then(Value::as_u64).unwrap_or(default)
    }

    pub fn get_str(&self, key: &str, default: &str) -> String {
        self.0.get(key).and_then(Value::as_str).unwrap_or(default).to_string()
    }

    /// 约定键：所有插件都有 enabled，缺省为 true。
    pub fn enabled(&self) -> bool {
        self.get_bool("enabled", true)
    }
}
```

- [ ] **Step 5: 运行测试确认通过**

```bash
cargo test -p talksage-plugins --lib registry
```

预期：`test result: ok. 3 passed`。

- [ ] **Step 6: 提交**

```bash
git add crates/talksage-plugins/src/registry.rs crates/talksage-plugins/src/lib.rs crates/talksage-plugins/Cargo.toml
git commit -m "feat(plugins): PluginConfig as the plugin configuration carrier"
```

---

## Task 3: 钩子 trait 与 `HookRegistry`

**Files:**
- Modify: `crates/talksage-plugins/src/registry.rs`

- [ ] **Step 1: 写失败的测试**

在 `registry.rs` 末尾追加：

```rust
#[cfg(test)]
mod hook_tests {
    use super::*;
    use std::sync::Arc;
    use talksage_core::DomainEvent;

    /// 测试替身：吞掉文本等于 drop_text 的 final 段。
    struct DropByText(&'static str);
    impl EventFilter for DropByText {
        fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
            match &ev {
                DomainEvent::Segment { text, .. } if text == self.0 => None,
                _ => Some(ev),
            }
        }
    }

    /// 测试替身：给文本加后缀，用来验证链式顺序。
    struct AppendSuffix(&'static str);
    impl EventFilter for AppendSuffix {
        fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
            match ev {
                DomainEvent::Segment { speaker_id, speaker_label, text, is_partial, ts_ms,
                                       duration_ms, rms, revision, start_sample, end_sample } => {
                    Some(DomainEvent::Segment {
                        speaker_id, speaker_label, text: format!("{text}{}", self.0),
                        is_partial, ts_ms, duration_ms, rms, revision, start_sample, end_sample,
                    })
                }
                other => Some(other),
            }
        }
    }

    fn seg(text: &str) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            text: text.into(),
            is_partial: false,
            ts_ms: 0,
            duration_ms: 500,
            rms: 0.1,
            revision: 0,
            start_sample: 0,
            end_sample: 8000,
        }
    }

    #[test]
    fn filters_apply_in_registration_order() {
        let mut hooks = HookRegistry::default();
        hooks.add_filter(Arc::new(AppendSuffix("-a")));
        hooks.add_filter(Arc::new(AppendSuffix("-b")));
        let out = hooks.apply_filters(seg("x")).expect("不应被吞掉");
        match out {
            DomainEvent::Segment { text, .. } => assert_eq!(text, "x-a-b", "应按注册顺序依次施加"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_filter_returning_none_swallows_the_event_and_stops_the_chain() {
        let mut hooks = HookRegistry::default();
        hooks.add_filter(Arc::new(DropByText("x")));
        hooks.add_filter(Arc::new(AppendSuffix("-never")));
        assert!(hooks.apply_filters(seg("x")).is_none(), "被吞掉的事件不应继续");
        // 不匹配的事件应原样穿过整条链
        let out = hooks.apply_filters(seg("y")).expect("不应被吞掉");
        match out {
            DomainEvent::Segment { text, .. } => assert_eq!(text, "y-never"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_registry_passes_everything_through() {
        let hooks = HookRegistry::default();
        assert!(hooks.apply_filters(seg("x")).is_some());
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p talksage-plugins --lib registry 2>&1 | grep -E "^error" | head -5
```

预期：`cannot find type HookRegistry` / `cannot find trait EventFilter`。

- [ ] **Step 3: 实现 trait 与注册表**

在 `registry.rs` 的 `PluginConfig` 之后插入：

```rust
use std::sync::Arc;
use talksage_core::{DomainEvent, TranscriptSegment};
use crate::PluginContext;

/// 快路径钩子：每个事件都过。
///
/// 签名里既没有 Result 也没有 PluginContext —— 这是刻意的：filter 必须是
/// 纯函数、不可失败、不可阻塞。想做 IO 或会失败的活，去 SegmentObserver。
pub trait EventFilter: Send + Sync {
    /// 返回 None 表示吞掉该事件：既不进 sink，也不触发 observer。
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent>;
}

/// 慢路径钩子：committed 段触发。
/// skeleton 同步、本地、无 HTTP；run 在独立线程，可含 LLM。
pub trait SegmentObserver: Send + Sync {
    fn name(&self) -> &'static str;
    fn should_trigger(&self, seg: &TranscriptSegment) -> bool;
    /// 是否消费 hypothesis（partial）。默认 false：只处理 committed。
    fn accepts_speculative(&self) -> bool {
        false
    }
    fn skeleton(&self, seg: &TranscriptSegment) -> Option<DomainEvent>;
    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent>;
}

/// 插件：拥有身份与默认配置，在 register() 里把自己挂进钩子。
/// 插件不拥有注册表，只能注册进去（对应 Cordis 的 seam 模型）。
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn default_config(&self) -> PluginConfig;
    fn register(&self, cfg: &PluginConfig, hooks: &mut HookRegistry);
}

/// 钩子集合。顺序即执行顺序。
#[derive(Default, Clone)]
pub struct HookRegistry {
    filters: Vec<Arc<dyn EventFilter>>,
    observers: Vec<Arc<dyn SegmentObserver>>,
}

impl HookRegistry {
    pub fn add_filter(&mut self, f: Arc<dyn EventFilter>) {
        self.filters.push(f);
    }

    pub fn add_observer(&mut self, o: Arc<dyn SegmentObserver>) {
        self.observers.push(o);
    }

    pub fn observers(&self) -> &[Arc<dyn SegmentObserver>] {
        &self.observers
    }

    pub fn filter_count(&self) -> usize {
        self.filters.len()
    }

    /// 依次施加 filter；任一返回 None 即吞掉并中断链条。
    pub fn apply_filters(&self, ev: DomainEvent) -> Option<DomainEvent> {
        self.filters.iter().try_fold(ev, |e, f| f.filter(e))
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p talksage-plugins --lib registry
```

预期：`test result: ok. 6 passed`（3 个配置 + 3 个钩子）。

- [ ] **Step 5: 提交**

```bash
git add crates/talksage-plugins/src/registry.rs
git commit -m "feat(plugins): hook traits and HookRegistry

EventFilter 的签名刻意不给 Result 和 PluginContext —— 用类型堵死在
热路径做 IO 或失败重试。"
```

---

## Task 4: `AnalyzerPlugin` 改名为 `SegmentObserver`

现有三个插件（term_explainer / translator / brief_retriever）实现的是 `AnalyzerPlugin`，与新的 `SegmentObserver` 签名完全一致。这一步只改名，不改行为。

**Files:**
- Modify: `crates/talksage-plugins/src/lib.rs`
- Modify: `crates/talksage-plugins/src/term_explainer.rs`
- Modify: `crates/talksage-plugins/src/translator.rs`
- Modify: `crates/talksage-plugins/src/brief_retriever.rs`
- Modify: `crates/talksage-pipeline/src/lib.rs`
- Modify: `crates/talksage-pipeline/src/service.rs`
- Modify: `crates/talksage-pipeline/tests/pipeline_live.rs`

- [ ] **Step 1: 删除旧 trait，改为从 registry 再导出**

编辑 `crates/talksage-plugins/src/lib.rs`：删除整个 `pub trait AnalyzerPlugin { ... }` 定义，改为：

```rust
pub mod registry;
pub use registry::{EventFilter, HookRegistry, Plugin, PluginConfig, SegmentObserver};

/// 过渡别名：老代码仍可用 AnalyzerPlugin 这个名字。
/// 阶段 3 迁移完 observer 后删除。
pub use registry::SegmentObserver as AnalyzerPlugin;
```

- [ ] **Step 2: 批量改 impl 名**

```bash
cd /Users/robot/projects/talk-sage
grep -rl "impl AnalyzerPlugin for" crates | xargs sed -i '' 's/impl AnalyzerPlugin for/impl SegmentObserver for/g'
grep -rn "impl SegmentObserver for" crates
```

预期输出三行，分别在 `term_explainer.rs`、`translator.rs`、`brief_retriever.rs`。

- [ ] **Step 3: 编译并跑全量测试，确认零行为变化**

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export SHERPA_ONNX_ARCHIVE_DIR="$PWD/.tools/sherpa-onnx-archives"
export TALKSAGE_MODELS_DIR="$PWD/models" TALKSAGE_REQUIRE_MODELS=1
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
```

预期：无输出（全绿）。特征化测试也必须过 —— 改名不该产生任何事件差异。若 golden 报差异，说明改名过程中动了行为，回退重来。

- [ ] **Step 4: 提交**

```bash
git add -A
git commit -m "refactor(plugins): rename AnalyzerPlugin to SegmentObserver

纯改名，零行为变化：特征化 golden 不变。AnalyzerPlugin 暂留为别名，
阶段 3 迁移完 observer 后删除。"
```

---

## Task 5: `short_segment` 插件

**Files:**
- Create: `crates/talksage-plugins/src/short_segment.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`

- [ ] **Step 1: 写失败的测试**

创建 `crates/talksage-plugins/src/short_segment.rs`，先只写测试：

```rust
//! short_segment：final 段时长低于阈值时吞掉（噪音短段抑制）。
//!
//! 迁移自 pipeline/src/lib.rs:646 —— 原实现在 StreamWorker::finish_speech
//! 里提前 return，同时抑制事件与插件。作为产生点 filter，语义等价。

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use talksage_core::DomainEvent;

    fn seg(duration_ms: u64, is_partial: bool) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            text: "喂".into(),
            is_partial,
            ts_ms: 1000,
            duration_ms,
            rms: 0.2,
            revision: 0,
            start_sample: 0,
            end_sample: 16000,
        }
    }

    fn filter_with(min_ms: u64) -> ShortSegmentFilter {
        ShortSegmentFilter { min_ms }
    }

    #[test]
    fn drops_final_segments_shorter_than_threshold() {
        assert!(filter_with(300).filter(seg(120, false)).is_none());
    }

    #[test]
    fn keeps_final_segments_at_or_above_threshold() {
        assert!(filter_with(300).filter(seg(300, false)).is_some(), "等于阈值应保留");
        assert!(filter_with(300).filter(seg(800, false)).is_some());
    }

    #[test]
    fn zero_threshold_disables_the_filter() {
        assert!(filter_with(0).filter(seg(1, false)).is_some());
    }

    #[test]
    fn never_touches_partials_or_other_events() {
        // partial 段时长恒为 0，绝不能被当成短段吞掉
        assert!(filter_with(300).filter(seg(0, true)).is_some());
        let level = DomainEvent::Level { mic_rms: 0.1, loopback_rms: 0.0 };
        assert!(filter_with(300).filter(level).is_some());
    }

    #[test]
    fn plugin_registers_a_filter_and_reads_min_ms_from_config() {
        let p = ShortSegmentPlugin;
        assert_eq!(p.id(), "short_segment");
        let mut cfg = p.default_config();
        cfg.merge(&json!({"min_ms": 500}));
        let mut hooks = HookRegistry::default();
        p.register(&cfg, &mut hooks);
        assert_eq!(hooks.filter_count(), 1);
        assert!(hooks.apply_filters(seg(400, false)).is_none(), "400ms < 配置的 500ms 应被吞");
    }
}
```

- [ ] **Step 2: 在 lib.rs 挂载模块并运行测试确认失败**

`crates/talksage-plugins/src/lib.rs` 加 `pub mod short_segment;`，然后：

```bash
cargo test -p talksage-plugins --lib short_segment 2>&1 | grep -E "^error" | head -5
```

预期：`cannot find type ShortSegmentFilter` / `cannot find type ShortSegmentPlugin`。

- [ ] **Step 3: 实现**

在 `short_segment.rs` 的测试模块之前插入：

```rust
use std::sync::Arc;

use serde_json::json;
use talksage_core::DomainEvent;

use crate::registry::{EventFilter, HookRegistry, Plugin, PluginConfig};

/// 默认最短提交时长（ms）。0 = 关闭。
const DEFAULT_MIN_MS: u64 = 0;

pub struct ShortSegmentFilter {
    pub min_ms: u64,
}

impl EventFilter for ShortSegmentFilter {
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
        if self.min_ms == 0 {
            return Some(ev);
        }
        if let DomainEvent::Segment { is_partial: false, duration_ms, speaker_label, text, .. } = &ev {
            if *duration_ms < self.min_ms {
                log::info!(
                    "短段丢弃[{}]: 时长={duration_ms}ms < 最短提交={}ms 文本={}",
                    speaker_label,
                    self.min_ms,
                    text.chars().take(40).collect::<String>(),
                );
                return None;
            }
        }
        Some(ev)
    }
}

pub struct ShortSegmentPlugin;

impl Plugin for ShortSegmentPlugin {
    fn id(&self) -> &'static str {
        "short_segment"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true, "min_ms": DEFAULT_MIN_MS }))
    }

    fn register(&self, cfg: &PluginConfig, hooks: &mut HookRegistry) {
        hooks.add_filter(Arc::new(ShortSegmentFilter {
            min_ms: cfg.get_u64("min_ms", DEFAULT_MIN_MS),
        }));
    }
}
```

（`log` 依赖已在 Task 2 Step 1 加好，此处无需再动 Cargo.toml。）

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p talksage-plugins --lib short_segment
```

预期：`test result: ok. 5 passed`。

- [ ] **Step 5: 提交**

```bash
git add crates/talksage-plugins/src/short_segment.rs crates/talksage-plugins/src/lib.rs crates/talksage-plugins/Cargo.toml
git commit -m "feat(plugins): short_segment filter

从 StreamWorker 剥出来后第一次可以脱离真实 ASR 单独测试 ——
此前只能靠集成测试间接覆盖。"
```

---

## Task 6: `cross_stream_dedup` 插件

**Files:**
- Create: `crates/talksage-plugins/src/cross_stream_dedup.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`

- [ ] **Step 1: 写失败的测试**

创建 `crates/talksage-plugins/src/cross_stream_dedup.rs`，先只写测试：

```rust
//! cross_stream_dedup：双流（麦克风 + 系统回环）把同一句话各识别一次，
//! 只保留先到的那份。
//!
//! 迁移自 pipeline/src/lib.rs:812 的 emit 包装。判定逻辑复用
//! talksage_core::is_echo_duplicate。

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::DomainEvent;

    fn seg(speaker_id: u32, text: &str, ts_ms: u64) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id,
            speaker_label: if speaker_id == 0 { "我".into() } else { "客户".into() },
            text: text.into(),
            is_partial: false,
            ts_ms,
            duration_ms: 800,
            rms: 0.2,
            revision: 0,
            start_sample: 0,
            end_sample: 12800,
        }
    }

    #[test]
    fn keeps_the_first_copy_and_drops_the_cross_stream_echo() {
        let f = CrossStreamDedupFilter::default();
        assert!(f.filter(seg(0, "我们下周确认交期", 1000)).is_some(), "先到的应保留");
        assert!(
            f.filter(seg(1, "我们下周确认交期", 1200)).is_none(),
            "另一条流的同一句话应被吞掉"
        );
    }

    #[test]
    fn same_stream_repetition_is_not_an_echo() {
        // 同一说话人真的说了两遍，不是双录，必须保留
        let f = CrossStreamDedupFilter::default();
        assert!(f.filter(seg(0, "好的", 1000)).is_some());
        assert!(f.filter(seg(0, "好的", 1200)).is_some(), "同流重复不是回声");
    }

    #[test]
    fn different_text_from_the_other_stream_passes_through() {
        let f = CrossStreamDedupFilter::default();
        assert!(f.filter(seg(0, "我们下周确认交期", 1000)).is_some());
        assert!(f.filter(seg(1, "完全不同的一句话", 1200)).is_some());
    }

    #[test]
    fn partials_are_never_deduped() {
        let f = CrossStreamDedupFilter::default();
        let mut p = seg(0, "重复", 1000);
        if let DomainEvent::Segment { is_partial, .. } = &mut p {
            *is_partial = true;
        }
        assert!(f.filter(p.clone()).is_some());
        assert!(f.filter(p).is_some(), "partial 不参与去重");
    }

    #[test]
    fn history_is_bounded() {
        let f = CrossStreamDedupFilter::default();
        for i in 0..100 {
            let _ = f.filter(seg(0, &format!("句子{i}"), 1000 + i as u64 * 100));
        }
        assert!(f.history_len() <= HISTORY_CAP, "历史窗口必须有界");
    }

    #[test]
    fn plugin_registers_one_filter() {
        let p = CrossStreamDedupPlugin;
        assert_eq!(p.id(), "cross_stream_dedup");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &mut hooks);
        assert_eq!(hooks.filter_count(), 1);
    }
}
```

- [ ] **Step 2: 挂载模块并运行测试确认失败**

`lib.rs` 加 `pub mod cross_stream_dedup;`，然后：

```bash
cargo test -p talksage-plugins --lib cross_stream_dedup 2>&1 | grep -E "^error" | head -5
```

预期：`cannot find type CrossStreamDedupFilter`。

- [ ] **Step 3: 实现**

在测试模块之前插入：

```rust
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::json;
use talksage_core::DomainEvent;

use crate::registry::{EventFilter, HookRegistry, Plugin, PluginConfig};

/// 回声比对的历史窗口容量（条）。
pub const HISTORY_CAP: usize = 32;

/// 跨流回声去重。内部有可变历史，因此用 Mutex —— filter 签名是 &self。
#[derive(Default)]
pub struct CrossStreamDedupFilter {
    /// (speaker_id, text, ts_ms)
    recent: Mutex<VecDeque<(u32, String, u64)>>,
}

impl CrossStreamDedupFilter {
    pub fn history_len(&self) -> usize {
        self.recent.lock().unwrap().len()
    }
}

impl EventFilter for CrossStreamDedupFilter {
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
        let DomainEvent::Segment {
            speaker_id, speaker_label, text, is_partial: false, ts_ms, ..
        } = &ev
        else {
            return Some(ev);
        };
        let mut recent = self.recent.lock().unwrap();
        let is_echo = recent.iter().any(|(sp, t, ts)| {
            *sp != *speaker_id && talksage_core::is_echo_duplicate(t, text, ts_ms.saturating_sub(*ts))
        });
        if is_echo {
            log::info!(
                "跨流回显去重: 丢弃[{}] 文本={}（与另一条流重复）",
                speaker_label,
                text.chars().take(40).collect::<String>()
            );
            return None;
        }
        recent.push_back((*speaker_id, text.clone(), *ts_ms));
        if recent.len() > HISTORY_CAP {
            recent.pop_front();
        }
        drop(recent);
        Some(ev)
    }
}

pub struct CrossStreamDedupPlugin;

impl Plugin for CrossStreamDedupPlugin {
    fn id(&self) -> &'static str {
        "cross_stream_dedup"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true }))
    }

    fn register(&self, _cfg: &PluginConfig, hooks: &mut HookRegistry) {
        hooks.add_filter(Arc::new(CrossStreamDedupFilter::default()));
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p talksage-plugins --lib cross_stream_dedup
```

预期：`test result: ok. 6 passed`。

- [ ] **Step 5: 提交**

```bash
git add crates/talksage-plugins/src/cross_stream_dedup.rs crates/talksage-plugins/src/lib.rs
git commit -m "feat(plugins): cross_stream_dedup filter"
```

---

## Task 7: `builtin_plugins()` 中心表与顺序不变量

**Files:**
- Create: `crates/talksage-plugins/src/builtin.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`

- [ ] **Step 1: 写失败的测试**

创建 `crates/talksage-plugins/src/builtin.rs`，先只写测试：

```rust
//! 内置插件中心表。列表顺序即钩子执行顺序。

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn plugin_ids_are_unique() {
        let plugins = builtin_plugins();
        let mut seen = std::collections::HashSet::new();
        for p in &plugins {
            assert!(seen.insert(p.id()), "重复的插件 id: {}", p.id());
        }
    }

    #[test]
    fn every_plugin_has_a_parsable_default_config_with_enabled() {
        for p in builtin_plugins() {
            let cfg = p.default_config();
            assert!(
                cfg.as_value().get("enabled").is_some(),
                "插件 {} 的默认配置缺少 enabled 键",
                p.id()
            );
        }
    }

    /// 设计 §3.4 S2：short_segment 必须排在 cross_stream_dedup 之前
    /// —— 便宜的先跑，且 dedup 需要看两条流的历史。
    #[test]
    fn short_segment_is_ordered_before_cross_stream_dedup() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        let short = ids.iter().position(|id| *id == "short_segment").expect("缺少 short_segment");
        let dedup = ids.iter().position(|id| *id == "cross_stream_dedup").expect("缺少 cross_stream_dedup");
        assert!(short < dedup, "short_segment 必须排在 cross_stream_dedup 之前，实际顺序: {ids:?}");
    }

    #[test]
    fn build_registry_skips_disabled_plugins() {
        let mut overrides = HashMap::new();
        overrides.insert("cross_stream_dedup".to_string(), serde_json::json!({"enabled": false}));
        let hooks = build_registry(&builtin_plugins(), &overrides);
        let all = build_registry(&builtin_plugins(), &HashMap::new());
        assert_eq!(hooks.filter_count() + 1, all.filter_count(), "关掉一个插件应少一个 filter");
    }

    #[test]
    fn build_registry_applies_user_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("short_segment".to_string(), serde_json::json!({"min_ms": 400}));
        let hooks = build_registry(&builtin_plugins(), &overrides);
        let short = DomainEvent::Segment {
            speaker_id: 0, speaker_label: "我".into(), text: "喂".into(),
            is_partial: false, ts_ms: 0, duration_ms: 200, rms: 0.1,
            revision: 0, start_sample: 0, end_sample: 3200,
        };
        assert!(hooks.apply_filters(short).is_none(), "200ms < 覆盖后的 400ms 应被吞");
    }
}
```

- [ ] **Step 2: 挂载模块并运行测试确认失败**

`lib.rs` 加 `pub mod builtin;` 与 `pub use builtin::{build_registry, builtin_plugins};`，然后：

```bash
cargo test -p talksage-plugins --lib builtin 2>&1 | grep -E "^error" | head -5
```

预期：`cannot find function builtin_plugins`。

- [ ] **Step 3: 实现**

在测试模块之前插入：

```rust
use std::collections::HashMap;

use serde_json::Value;
use talksage_core::DomainEvent;

use crate::cross_stream_dedup::CrossStreamDedupPlugin;
use crate::registry::{HookRegistry, Plugin};
use crate::short_segment::ShortSegmentPlugin;

/// 内置插件清单。
///
/// **顺序即执行顺序**（设计 §3.4 S2）。改动顺序前先看 builtin.rs 里的
/// 顺序不变量测试 —— 它锁住了有依赖关系的相对位置。
pub fn builtin_plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        // filter：便宜的先跑；dedup 需要看两条流的历史，必须在 short_segment 之后
        Box::new(ShortSegmentPlugin),
        Box::new(CrossStreamDedupPlugin),
    ]
}

/// 按配置装配钩子。overrides 的键是插件 id。
pub fn build_registry(
    plugins: &[Box<dyn Plugin>],
    overrides: &HashMap<String, Value>,
) -> HookRegistry {
    let mut hooks = HookRegistry::default();
    for p in plugins {
        let mut cfg = p.default_config();
        if let Some(user) = overrides.get(p.id()) {
            cfg.merge(user);
        }
        if !cfg.enabled() {
            log::debug!("插件[{}] 已禁用，跳过注册", p.id());
            continue;
        }
        p.register(&cfg, &mut hooks);
    }
    hooks
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p talksage-plugins --lib builtin
```

预期：`test result: ok. 5 passed`。

- [ ] **Step 5: 提交**

```bash
git add crates/talksage-plugins/src/builtin.rs crates/talksage-plugins/src/lib.rs
git commit -m "feat(plugins): builtin_plugins registry with order invariants

顺序不变量由测试锁住：short_segment 必须在 cross_stream_dedup 之前。"
```

---

## Task 8: 把 filter 链接进管道，删除旧的硬编码实现

这是风险最高的一步：动热路径，且包含一次已决策的行为变更（跨流回声重复段从此不再触发插件，见设计 §3.3）。

**Files:**
- Modify: `crates/talksage-pipeline/src/lib.rs`（`LivePipelineConfig`、`StreamWorker::finish_speech`、`run_loop` 的 emit 包装）
- Modify: `crates/talksage-pipeline/src/service.rs`
- Modify: `crates/talksage-pipeline/tests/pipeline_live.rs`
- Modify: `crates/talksage-pipeline/src/offline.rs`
- Modify: `crates/talksage-pipeline/tests/characterization.rs`

- [ ] **Step 1: 给 `LivePipelineConfig` 加 hooks 字段**

在 `crates/talksage-pipeline/src/lib.rs` 的 `LivePipelineConfig` 结构体里，把：

```rust
    pub min_commit_ms: u64,
```

替换为：

```rust
    /// 插件钩子（filter 链 + observer）。由 TalkSageService 用
    /// talksage_plugins::build_registry 装配。
    pub hooks: talksage_plugins::HookRegistry,
```

同时删除 `StreamWorker` 的 `min_commit_ms` 字段、构造参数与所有赋值点。

- [ ] **Step 2: 在产生点应用 filter 链**

在 `StreamWorker::finish_speech` 里，删除这段（原 `lib.rs:646` 附近）：

```rust
            // 最短提交时长：短段丢弃（噪音短段抑制，减少无效短段污染转写/历史）
            if self.min_commit_ms > 0 && duration_ms < self.min_commit_ms {
                log::info!(
                    "流[{}] 短段丢弃: 时长={duration_ms}ms < 最短提交={}ms 文本={}",
                    self.speaker_label,
                    self.min_commit_ms,
                    final_text.chars().take(40).collect::<String>(),
                );
                return;
            }
```

然后把原来的 `emit(...)` + `on_final(...)` 这一段（原 `lib.rs:701-715`）：

```rust
            emit(DomainEvent::Segment {
                speaker_id: seg.speaker_id,
                speaker_label: seg.speaker_label.clone(),
                text: seg.text.clone(),
                is_partial: false,
                ts_ms: seg.ts_ms,
                duration_ms: seg.duration_ms,
                rms: seg.rms,
                revision: 0,
                start_sample: self.seg_start_sample,
                end_sample,
            });
            if let Some(hook) = &self.on_final {
                hook(&seg);
            }
```

替换为：

```rust
            // filter 链在产生点施加：被吞掉的事件既不 emit，也不触发 observer。
            // 这一点必须保持——短段抑制原本就同时拦住两者。
            let ev = DomainEvent::Segment {
                speaker_id: seg.speaker_id,
                speaker_label: seg.speaker_label.clone(),
                text: seg.text.clone(),
                is_partial: false,
                ts_ms: seg.ts_ms,
                duration_ms: seg.duration_ms,
                rms: seg.rms,
                revision: 0,
                start_sample: self.seg_start_sample,
                end_sample,
            };
            let Some(ev) = self.hooks.apply_filters(ev) else {
                // 被吞掉：不计统计、不 emit、不触发 observer，但仍要收尾引擎状态
                self.last_partial.clear();
                if let Some(e) = &mut self.engine {
                    e.reset();
                }
                self.seg_audio.clear();
                return;
            };
            self.final_segments += 1;
            self.words += talksage_core::metrics::count_words(&final_text);
            if talksage_core::metrics::is_question_text(&final_text) {
                self.questions += 1;
            }
            emit(ev);
            if let Some(hook) = &self.on_final {
                hook(&seg);
            }
```

**统计口径**：原实现在短段处提前 `return`，而 `final_segments += 1`、`words`、`questions` 三处累加位于其后（`lib.rs:696-700`），所以短段本就不计数。上面的代码把这三处累加移到了 filter 之后，语义等价。**实现时必须把原位置（`lib.rs:696-700`）的这三行删掉**，否则会重复累加。

给 `StreamWorker` 加 `hooks: talksage_plugins::HookRegistry` 字段，构造时从 `cfg.hooks.clone()` 取。

> **关键：两条流必须共享同一个 dedup filter 实例。** `HookRegistry` 派生 `Clone`，克隆的是 `Arc<dyn EventFilter>`，因此 `cfg.hooks.clone()` 给两个 `StreamWorker` 的是**同一个** `CrossStreamDedupFilter`，其内部 `Mutex<VecDeque>` 历史被共享 —— 这正是跨流去重能工作的前提。若实现时改成每个 worker 各建一份注册表，去重会静默失效，而 `cross_stream_echo_dedup_keeps_single_copy` 测试会抓到。

- [ ] **Step 3: 删除 emit 包装里的去重**

在 `run_loop` 里删除 `recent_finals` 声明与 emit 包装内的整个去重块（原 `lib.rs:812-846`），保留 `seg_log` 与 metrics/nudge 部分不动（那是阶段 3 的事）。

- [ ] **Step 4: service.rs 用 build_registry 装配**

在 `crates/talksage-pipeline/src/service.rs` 的 `build_live_config` 里，把 `min_commit_ms: snapshot.audio.min_segment_ms` 替换为：

```rust
        // 阶段 2 过渡：filter 类插件的配置暂时仍从既有具名字段翻译过来，
        // 阶段 5 换成通用 [plugins.<id>] 表后这段删除。
        let mut plugin_overrides: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        plugin_overrides.insert(
            "short_segment".into(),
            serde_json::json!({ "min_ms": snapshot.audio.min_segment_ms }),
        );
        plugin_overrides.insert(
            "cross_stream_dedup".into(),
            serde_json::json!({ "enabled": true }),
        );
        let hooks = talksage_plugins::build_registry(&talksage_plugins::builtin_plugins(), &plugin_overrides);
```

并在返回的 `LivePipelineConfig` 里用 `hooks` 替换 `min_commit_ms`。

- [ ] **Step 5: 修测试与 offline.rs 的构造点**

`crates/talksage-pipeline/tests/pipeline_live.rs`、`crates/talksage-pipeline/tests/characterization.rs`、`crates/talksage-pipeline/src/offline.rs` 里所有 `min_commit_ms: N` 改为：

```rust
        hooks: talksage_plugins::build_registry(
            &talksage_plugins::builtin_plugins(),
            &std::collections::HashMap::from([(
                "short_segment".to_string(),
                serde_json::json!({ "min_ms": N }),
            )]),
        ),
```

（把 `N` 换成该处原有的数值。`characterization.rs` 的 `zh_pipeline` 函数签名里 `min_commit_ms` 参数保留，传进这里。）

- [ ] **Step 6: 编译并跑单流测试**

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export SHERPA_ONNX_ARCHIVE_DIR="$PWD/.tools/sherpa-onnx-archives"
export TALKSAGE_MODELS_DIR="$PWD/models" TALKSAGE_REQUIRE_MODELS=1
cargo test -p talksage-pipeline --test characterization
```

预期：**PASS，golden 无差异**。单流语料不涉及跨流去重，短段抑制语义未变，因此这一步不该有任何事件差异。若报差异，说明搬迁过程改了行为，先查清楚再往下。

- [ ] **Step 7: 跑集成测试，确认两个原有测试仍绿**

```bash
cargo test -p talksage-pipeline --test pipeline_live -- --nocapture 2>&1 | grep -E "^test |test result|去重|短段"
```

预期：8 个测试全过，其中 `min_commit_ms_suppresses_short_segments` 与 `cross_stream_echo_dedup_keeps_single_copy` 必须过 —— 它们是这次搬迁的回归网。

- [ ] **Step 8: 全量测试**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
cargo clippy --workspace 2>&1 | grep -E "short_segment|cross_stream|hooks" | head
```

预期：第一条无输出；第二条无输出（新代码无 clippy 提示）。

- [ ] **Step 9: 提交**

```bash
git add -A
git commit -m "refactor(pipeline): apply the filter chain at the production point

short_segment 与 cross_stream_dedup 从 StreamWorker 和 emit 包装里搬到
插件 filter 链。filter 在 emit 与 on_final 之前施加，被吞掉的事件两条
路都不走。

已决策的行为变更：跨流回声重复段从此不再触发插件。原实现里 on_final
紧跟 emit 调用、绕过了 emit 包装的去重，导致重复段仍会触发术语/翻译/
简报插件（可能带重复的 LLM 调用）。见设计 §3.3。

pipeline/src/lib.rs 减少约 60 行。"
```

---

## Task 9: 收尾核对

- [ ] **Step 1: 确认 pipeline/lib.rs 确实瘦身**

```bash
wc -l crates/talksage-pipeline/src/lib.rs
```

预期：低于重构前的 1130 行（约 1070 行左右；大头的 metrics 调度是阶段 3 的事）。

- [ ] **Step 2: 确认加插件的成本已降低**

人工核对：新增一个 filter 类插件现在只需要
（a）在 `crates/talksage-plugins/src/` 加一个文件，
（b）在 `builtin.rs` 的 `builtin_plugins()` 里加一行。
不需要改 `pipeline/src/lib.rs`。若不满足，说明接缝没做干净。

- [ ] **Step 3: 全量验证**

```bash
export TALKSAGE_MODELS_DIR="$PWD/models" TALKSAGE_REQUIRE_MODELS=1
cargo test --workspace 2>&1 | grep -cE "test result: ok"
cd web && npx vitest run 2>&1 | tail -3 && cd ..
```

预期：Rust 全绿零跳过；前端 41 passed。

- [ ] **Step 4: 更新设计文档的阶段勾选**

在 `docs/superpowers/specs/2026-08-20-everything-is-a-plugin-design.md` 的 §7 表格里，把阶段 1、2 标记为已完成，并注明对应提交。

```bash
git add docs/superpowers/specs/2026-08-20-everything-is-a-plugin-design.md
git commit -m "docs: mark plugin refactor stages 1-2 as done"
```

---

## 阶段 3–5 的入口

本计划完成后，接缝已验证：`Plugin` / `EventFilter` / `HookRegistry` 可用，两个 filter 已在生产路径上跑。后续计划另行成文：

- **阶段 3**：`conversation_metrics` / `coaching_nudge` 迁为 `SegmentObserver`，删除 `AnalyzerPlugin` 过渡别名
- **阶段 4**：`SessionFinalizer` trait + `session_quality` / `webhook` / `markdown_export` / `trio_notes`，server 与 tauri 导出合一
- **阶段 5**：配置换通用 `[plugins.<id>]` 表 + `/plugins` 元数据端点 + 设置 UI 自动生成
