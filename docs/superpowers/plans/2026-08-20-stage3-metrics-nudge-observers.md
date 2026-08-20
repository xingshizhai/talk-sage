# 阶段 3：会话指标与教练提示迁为 Observer

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `conversation_metrics` 与 `coaching_nudge` 的调度从 `run_loop` 的 emit 包装搬到 `SegmentObserver` 钩子上，并删除 `AnalyzerPlugin` 过渡别名。

**Architecture:** `SegmentObserver::skeleton` 签名由 `Option<DomainEvent>` 改为 `Vec<DomainEvent>`，使一个 observer 能在一段上发出多个事件。新增 `conversation_metrics` 插件，内部持有 `seg_log` 与 `NudgeEngine`，一次计算同时产出 `Metrics` 与（可选的）`Nudge`。

**Tech Stack:** Rust 2021，现有 `talksage-plugins` / `talksage-pipeline`，不新增依赖、不新增 crate。

**对应设计：** [2026-08-20-everything-is-a-plugin-design.md](../specs/2026-08-20-everything-is-a-plugin-design.md) 阶段 3。前置：阶段 1–2 已合并（`3f53bb0`）。

---

## 两个已决策的前提

**D1. `skeleton` 返回 `Vec<DomainEvent>`。** 现签名一次只能返回一个事件，而指标插件每段要发 `Metrics`，可能再发 `Nudge`。改为 `Vec`（空向量 = 不发）。`term_explainer` / `translator` / `brief_retriever` 三个现有实现需机械改造。

**D2. 事件顺序会变，golden 需更新。** 现状 metrics/nudge 在 emit 包装里，**先于** `Segment` 事件发出；observer 在 `on_final` 派发，**晚于** `emit`。搬迁后顺序由 `metrics → final` 变为 `final → metrics`。

已核实前端不依赖旧顺序（`App.tsx:184-189` 中 `setMetrics` / `setNudges` 均为独立状态写入，无跨事件依赖）。新顺序在语义上也更合理：先有段，再有基于段的统计。

**这是本计划唯一允许的 golden 变更**，且必须严格限定为「`metrics` 行从 `final` 行之前移到之后」。任何其他差异（段数、文本、时长、stats 行）都是回归。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| 修改 `crates/talksage-plugins/src/registry.rs` | `SegmentObserver::skeleton` 改返回 `Vec<DomainEvent>` |
| 修改 `crates/talksage-plugins/src/term_explainer.rs` | 适配新签名 |
| 修改 `crates/talksage-plugins/src/translator.rs` | 适配新签名 |
| 修改 `crates/talksage-plugins/src/brief_retriever.rs` | 适配新签名 |
| 创建 `crates/talksage-plugins/src/conversation_metrics.rs` | 指标 + 提示插件（含 `seg_log`、`NudgeEngine`） |
| 修改 `crates/talksage-plugins/src/builtin.rs` | 注册新插件 |
| 修改 `crates/talksage-plugins/src/lib.rs` | 挂模块；删除 `AnalyzerPlugin` 别名 |
| 修改 `crates/talksage-pipeline/src/lib.rs` | `make_on_final` 适配 `Vec`；删除 emit 包装里的 metrics/nudge 块 |
| 修改 `crates/talksage-pipeline/src/service.rs` | 观察者由注册表提供，不再手工装配 |
| 修改 `crates/talksage-pipeline/tests/golden/zh_single_stream.txt` | 仅顺序变更 |

---

## Task 1: `skeleton` 改为返回 `Vec<DomainEvent>`

纯签名改造，零行为变化。golden 必须保持不变。

**Files:**
- Modify: `crates/talksage-plugins/src/registry.rs`
- Modify: `crates/talksage-plugins/src/{term_explainer,translator,brief_retriever}.rs`
- Modify: `crates/talksage-pipeline/src/lib.rs`（`make_on_final`）

- [ ] **Step 1: 改 trait 签名**

`registry.rs` 中：

```rust
    /// 本地即时骨架（同步、无 HTTP）。返回多个事件：一段上可能同时产出
    /// 指标与提示。空向量 = 不发。
    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent>;
```

- [ ] **Step 2: 运行编译，确认三处实现报错**

```bash
cd /Users/robot/projects/talk-sage && source .env-worktree 2>/dev/null || export PATH="/opt/homebrew/opt/rustup/bin:$PATH" SHERPA_ONNX_ARCHIVE_DIR="$PWD/.tools/sherpa-onnx-archives" TALKSAGE_MODELS_DIR="$PWD/models"
cargo build -p talksage-plugins 2>&1 | grep -E "^error" | head
```

预期：三个 `expected Vec<DomainEvent>, found Option<DomainEvent>`，分别在 term_explainer / translator / brief_retriever。

- [ ] **Step 3: 适配三个现有实现**

`translator.rs` 与 `brief_retriever.rs` 现状都只有一行 `None`，改为：

```rust
    fn skeleton(&self, _seg: &TranscriptSegment) -> Vec<DomainEvent> {
        Vec::new()
    }
```

`term_explainer.rs` 有实际逻辑，只改返回包装、不动判定：

```rust
    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let acronyms = self.unseen_acronyms(&seg.text);
        if acronyms.is_empty() {
            return Vec::new();          // 原 return None
        }
        let result_id = format!("term-{}", now() as u64);
        *self.pending_result_id.lock().unwrap() = Some(result_id.clone());
        let content = if acronyms.len() == 1 {
            format!("{} = …", acronyms[0])
        } else {
            format!("{} = …", acronyms.join("、"))
        };
        vec![DomainEvent::Term {                // 原 Some(...)
            result_id,
            status: ResultStatus::Skeleton,
            content,
        }]
    }
```

注意 `*self.pending_result_id.lock().unwrap() = Some(result_id.clone())` 这行的 `Some` 是 `Option<String>` 字段，**不要**跟着改。

- [ ] **Step 4: 适配 `make_on_final` 的派发**

`crates/talksage-pipeline/src/lib.rs` 中，把

```rust
            if let Some(skel) = plugin.skeleton(seg) {
                emit(skel);
            }
```

改为

```rust
            for skel in plugin.skeleton(seg) {
                emit(skel);
            }
```

- [ ] **Step 5: 全量测试 + golden 必须不变**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
git diff --exit-code crates/talksage-pipeline/tests/golden/ && echo "golden 未变 ✓"
```

预期：第一条无输出；golden 未变。**这一步是纯签名改造，golden 变了就说明改错了。**

- [ ] **Step 6: 提交**

```bash
git add crates/talksage-plugins/src/registry.rs crates/talksage-plugins/src/term_explainer.rs crates/talksage-plugins/src/translator.rs crates/talksage-plugins/src/brief_retriever.rs crates/talksage-pipeline/src/lib.rs
git commit -m "refactor(plugins): skeleton 返回 Vec<DomainEvent>

一段上可能同时产出多个事件（指标 + 提示），Option 表达不了。
纯签名改造，三个现有插件机械适配，特征化 golden 不变。"
```

---

## Task 2: `conversation_metrics` 插件

**Files:**
- Create: `crates/talksage-plugins/src/conversation_metrics.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`（加 `pub mod conversation_metrics;`）

- [ ] **Step 1: 写失败的测试**

创建 `crates/talksage-plugins/src/conversation_metrics.rs`，先只写测试：

```rust
//! conversation_metrics：跨段累计会话指标，并按规则产出会中提示。
//!
//! 迁移自 pipeline/src/lib.rs 的 emit 包装。一次计算同时产出 Metrics 与
//! （可选的）Nudge —— 两者共享同一份 seg_log，不重复计算。

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::{DomainEvent, TranscriptSegment};

    fn seg(speaker_id: u32, label: &str, text: &str, ts_ms: u64) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id,
            speaker_label: label.into(),
            text: text.into(),
            is_partial: false,
            ts_ms,
            duration_ms: 1000,
            rms: 0.2,
        }
    }

    #[test]
    fn emits_metrics_for_every_final_segment() {
        let p = ConversationMetricsObserver::default();
        let evs = p.skeleton(&seg(0, "我", "我们下周确认交期", 1000));
        assert!(
            evs.iter().any(|e| matches!(e, DomainEvent::Metrics { .. })),
            "每个 final 段都应产出 Metrics: {evs:?}"
        );
    }

    #[test]
    fn metrics_accumulate_across_segments() {
        let p = ConversationMetricsObserver::default();
        p.skeleton(&seg(0, "我", "第一句", 1000));
        let evs = p.skeleton(&seg(1, "客户", "第二句", 2000));
        let m = evs
            .iter()
            .find_map(|e| match e {
                DomainEvent::Metrics { metrics } => Some(metrics),
                _ => None,
            })
            .expect("应有 Metrics");
        assert_eq!(
            m.segment_count_me + m.segment_count_them,
            2,
            "指标必须跨段累计，而不是只看当前段: {m:?}"
        );
    }

    #[test]
    fn ignores_partial_segments() {
        let p = ConversationMetricsObserver::default();
        let mut partial = seg(0, "我", "半句", 1000);
        partial.is_partial = true;
        assert!(!p.should_trigger(&partial), "partial 不应触发指标累计");
    }

    #[test]
    fn nudge_is_optional_not_emitted_every_segment() {
        // NudgeEngine 有 2 分钟冷却；单段不应立刻触发提示。
        let p = ConversationMetricsObserver::default();
        let evs = p.skeleton(&seg(0, "我", "你好", 1000));
        assert!(
            !evs.iter().any(|e| matches!(e, DomainEvent::Nudge { .. })),
            "首段不应立刻产出 Nudge（冷却未到）: {evs:?}"
        );
    }

    #[test]
    fn plugin_registers_one_observer() {
        let p = ConversationMetricsPlugin;
        assert_eq!(p.id(), "conversation_metrics");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &mut hooks);
        assert_eq!(hooks.observers().len(), 1);
    }
}
```

- [ ] **Step 2: 挂模块并运行，确认失败**

`lib.rs` 加 `pub mod conversation_metrics;`，然后：

```bash
cargo test -p talksage-plugins --lib conversation_metrics 2>&1 | grep -E "^error" | head -5
```

预期：`cannot find type ConversationMetricsObserver`。

- [ ] **Step 3: 实现**

在测试模块之前插入。搬迁自 `pipeline/src/lib.rs` 的 emit 包装块，保持逻辑不变：

```rust
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use talksage_core::{DomainEvent, NudgeEngine, TranscriptSegment};

use crate::registry::{HookRegistry, Plugin, PluginConfig, SegmentObserver};

/// 跨段累计的会话指标 + 提示引擎。
pub struct ConversationMetricsObserver {
    seg_log: Mutex<Vec<TranscriptSegment>>,
    nudge: Mutex<NudgeEngine>,
    /// 会话起点，用于 nudge 的 call_ms。
    started: Instant,
}

impl Default for ConversationMetricsObserver {
    fn default() -> Self {
        Self {
            seg_log: Mutex::new(Vec::new()),
            nudge: Mutex::new(NudgeEngine::default()),
            started: Instant::now(),
        }
    }
}

impl SegmentObserver for ConversationMetricsObserver {
    fn name(&self) -> &'static str {
        "conversation_metrics"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        !seg.is_partial
    }

    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let metrics = {
            let mut log = self.seg_log.lock().unwrap();
            log.push(seg.clone());
            talksage_core::compute_conversation_metrics(&log)
        };
        let mut out = vec![DomainEvent::Metrics {
            metrics: metrics.clone(),
        }];
        let call_ms = self.started.elapsed().as_millis() as u64;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if let Some(nudge) = self.nudge.lock().unwrap().evaluate(&metrics, call_ms, now_ms) {
            log::info!("会中提示[{:?}] {}", nudge.kind, nudge.message);
            out.push(DomainEvent::Nudge { nudge });
        }
        out
    }

    /// 纯本地计算，没有慢路径工作。
    fn run(&self, _seg: &TranscriptSegment, _ctx: &crate::PluginContext) -> Option<DomainEvent> {
        None
    }
}

pub struct ConversationMetricsPlugin;

impl Plugin for ConversationMetricsPlugin {
    fn id(&self) -> &'static str {
        "conversation_metrics"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true }))
    }

    fn register(&self, _cfg: &PluginConfig, hooks: &mut HookRegistry) {
        hooks.add_observer(Arc::new(ConversationMetricsObserver::default()));
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p talksage-plugins --lib conversation_metrics
```

预期：`test result: ok. 5 passed`。

- [ ] **Step 5: 提交**

```bash
git add crates/talksage-plugins/src/conversation_metrics.rs crates/talksage-plugins/src/lib.rs
git commit -m "feat(plugins): conversation_metrics observer

指标与提示共享同一份 seg_log，一次计算同时产出 Metrics 与可选的 Nudge。
此前它们在 pipeline 的 emit 包装里，与采集/VAD/ASR 挤在一个文件。"
```

---

## Task 3: 接进管道，删除旧调度

这一步会改变事件顺序，golden 需更新。

**Files:**
- Modify: `crates/talksage-pipeline/src/lib.rs`（删除 emit 包装里的 metrics/nudge 块）
- Modify: `crates/talksage-plugins/src/builtin.rs`（注册插件）
- Modify: `crates/talksage-pipeline/src/service.rs`（observer 改由注册表提供）
- Modify: `crates/talksage-pipeline/tests/golden/zh_single_stream.txt`

- [ ] **Step 1: 注册进中心表**

`builtin.rs` 的 `builtin_plugins()` 尾部追加：

```rust
        // observer：顺序无依赖，但放在 filter 之后便于阅读
        Box::new(ConversationMetricsPlugin),
```

并加 `use crate::conversation_metrics::ConversationMetricsPlugin;`。

- [ ] **Step 2: service.rs 让注册表提供 observer**

`build_live_config` 中，现有手工装配的三个分析插件（term/translator/brief）继续保留在 `cfg.plugins`（阶段 5 再统一），但要把注册表里的 observer 也并进去：

```rust
        // 注册表提供的 observer（阶段 3 起：conversation_metrics）
        for o in hooks.observers() {
            plugins.push(o.clone());
        }
```

放在 `plugins` 构建完成之后、`LivePipelineConfig` 构造之前。

- [ ] **Step 3: 删除 emit 包装里的旧调度**

`crates/talksage-pipeline/src/lib.rs` 的 `run_loop` 中，删除整个 metrics/nudge 包装块（从注释 `// 会话指标 + 实时提示` 到该 `emit` 闭包定义结束），恢复 `emit` 为直接传入的 sink。同时删掉不再使用的 `seg_log`、`nudge_engine`、`session_start` 局部变量与相关 `use`。

- [ ] **Step 4: 跑测试，预期 golden 出现顺序差异**

```bash
cargo test -p talksage-pipeline --test characterization 2>&1 | tail -20
```

预期：**FAILED**，diff 显示 `metrics` 行从 `final` 之前移到之后。

**逐行核对该 diff。** 允许的差异只有 `metrics` 行的位置。若出现下列任一，立即停止排查，不要更新 golden：
- `final` 行数量变化
- `final` 行的文本或 `duration_ms` 变化
- `stats` 行的段数变化
- `metrics` 行总数变化

- [ ] **Step 5: 确认差异符合预期后更新 golden**

```bash
TALKSAGE_UPDATE_GOLDEN=1 cargo test -p talksage-pipeline --test characterization
git diff crates/talksage-pipeline/tests/golden/
```

再次人工核对 diff 只含顺序变化。

- [ ] **Step 6: 全量测试**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
```

预期：无输出。特别确认 `pipeline_live` 中断言 Metrics 事件的测试（`file_input_produces_status_and_segments` 内有 Metrics 断言）仍绿。

- [ ] **Step 7: 提交**

```bash
git add crates/talksage-pipeline/src/lib.rs crates/talksage-plugins/src/builtin.rs crates/talksage-pipeline/src/service.rs crates/talksage-pipeline/tests/golden/zh_single_stream.txt
git commit -m "refactor(pipeline): 指标与提示改由 observer 派发

从 run_loop 的 emit 包装搬到 conversation_metrics observer。

事件顺序变更：metrics/nudge 此前在 emit 包装里、先于 Segment 事件发出；
observer 在 on_final 派发，因此改为后于 Segment。golden 相应更新——
diff 只含 metrics 行位置变化，段数/文本/时长/stats 均未变。

已核实前端不依赖旧顺序（App.tsx 中 setMetrics/setNudges 为独立状态
写入，无跨事件依赖）。新顺序语义上也更合理：先有段，再有基于段的统计。"
```

---

## Task 4: 删除 `AnalyzerPlugin` 过渡别名

**Files:**
- Modify: `crates/talksage-plugins/src/lib.rs`
- Modify: `crates/talksage-pipeline/src/lib.rs`、`src/service.rs`（改用 `SegmentObserver`）

- [ ] **Step 1: 删除别名**

`crates/talksage-plugins/src/lib.rs` 中删除：

```rust
/// 过渡别名：老代码仍可用 AnalyzerPlugin 这个名字。
pub use registry::SegmentObserver as AnalyzerPlugin;
```

- [ ] **Step 2: 编译，按报错逐处替换**

```bash
cargo build --workspace 2>&1 | grep -E "^error" | head
```

把所有 `AnalyzerPlugin` 引用改为 `SegmentObserver`（`pipeline/src/lib.rs` 的 `use` 与 `LivePipelineConfig.plugins` 字段类型、`service.rs` 的 `use`）。

- [ ] **Step 3: 全量测试 + golden 不变**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
git diff --exit-code crates/talksage-pipeline/tests/golden/ && echo "golden 未变 ✓"
```

纯改名，golden 必须不变。

- [ ] **Step 4: 提交**

```bash
git add -u crates/talksage-plugins/src/lib.rs crates/talksage-pipeline/src/lib.rs crates/talksage-pipeline/src/service.rs
git commit -m "refactor(plugins): 删除 AnalyzerPlugin 过渡别名

阶段 1 引入的兼容层，observer 迁移完成后不再需要。"
```

---

## Task 5: 收尾核对

- [ ] **Step 1: 行数**

```bash
wc -l crates/talksage-pipeline/src/lib.rs
```

阶段 2 结束时是 1119。删除 metrics/nudge 调度块（约 40 行）后应降到 1080 以下。**这是阶段 3 唯一以行数为参考的地方**，且只作观察，不作门槛。

- [ ] **Step 2: 确认 pipeline 不再引用具体 observer 类型**

```bash
grep -c "ConversationMetrics\|NudgeEngine\|compute_conversation_metrics" crates/talksage-pipeline/src/lib.rs
```

预期：0。

- [ ] **Step 3: 全量验证**

```bash
export TALKSAGE_REQUIRE_MODELS=1
cargo test --workspace 2>&1 | grep -cE "test result: ok"
(cd web && npx vitest run 2>&1 | grep -E "Tests +[0-9]")
cargo clippy --workspace --all-targets 2>&1 | grep -E "conversation_metrics|skeleton" | head
```

预期：Rust 全绿零跳过；前端 41 passed；clippy 新代码无提示。

- [ ] **Step 4: 更新设计文档**

在 `docs/superpowers/specs/2026-08-20-everything-is-a-plugin-design.md` §7 把阶段 3 标为完成。同时更新 §3.5 插件清单：`conversation_metrics` 与 `coaching_nudge` 合并为**一个**插件 `conversation_metrics`（同时产出 Metrics 与 Nudge，共享 seg_log 避免重复计算），并在 §3.1 记录 `skeleton` 返回值由 `Option` 改为 `Vec` 及原因。

```bash
git add docs/superpowers/specs/2026-08-20-everything-is-a-plugin-design.md
git commit -m "docs: 标记阶段 3 完成并校正插件清单"
```

---

## 阶段 4 的入口

阶段 3 完成后，`SessionFinalizer` 仍未实现（`HookRegistry` 无 `finalizers` 字段）。阶段 4 需要：新增该 trait 与 `FinalizeContext`、迁移 `session_quality` / `webhook` / `markdown_export` / `trio_notes`、合并 server 与 tauri 的两份导出实现。待阶段 3 落地后另行成文。
