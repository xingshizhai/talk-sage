# 阶段 4：会话收尾钩子（SessionFinalizer）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建立 `SessionFinalizer` 钩子，把 `finish()` 里硬编码的质量评估与 webhook 搬上去。

**Architecture:** `HookRegistry` 增加 `finalizers` 字段。`finish()` 停管道、落库、构造 `FinalizeContext`，然后依次调用 finalizer，逐个独立、互不阻塞。导出与纪要**不是** finalizer，且不存在需要处理的重复 —— 见下方范围修正。

**Tech Stack:** Rust 2021，现有 crate，不新增依赖、不新增 crate。

**对应设计：** [2026-08-20-everything-is-a-plugin-design.md](../specs/2026-08-20-everything-is-a-plugin-design.md) 阶段 4。前置：阶段 1–3 已合并（`ab7b7be`）。

---

## 一个必须先说清楚的范围修正

设计 §3.5 原把四样东西列为 Finalizer。核对代码后，**只有两样真的是**：

| | 真实触发方式 | `finish()` 调用次数 | 归属 |
|---|---|---|---|
| `session_quality` | 会话结束自动 | 内联在 `finish()` | ✅ Finalizer |
| `webhook` | 会话结束自动 | 内联在 `finish()` | ✅ Finalizer |
| `markdown_export` | 用户在历史页点击 | **0** | ❌ 按需 API |
| `trio_notes` | 用户在历史页点击 | **0** | ❌ 按需 API |

后两者是 `GET /session/{id}/export`、`POST /session/{id}/trio-notes` 及对应 Tauri 命令。把它们做成 finalizer，等于每次会话结束自动导出并调 LLM 烧 token，与「用户点击才生成」的现有产品行为相抵触。

设计文档还称它们「server 与 tauri 各一份实现」，这一条同样不成立 —— 见下方已取消的 Task 5。spec 两处均已加勘误。

因此本计划只做 finalizer 钩子（Task 1–4）。原拟的 Task 5「导出去重」在核对代码后取消 —— 见下方说明，那里并不存在重复实现。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| 修改 `crates/talksage-plugins/src/registry.rs` | `SessionFinalizer` trait、`FinalizeContext`、`HookRegistry.finalizers` |
| 创建 `crates/talksage-plugins/src/session_quality.rs` | 质量评估 finalizer |
| 创建 `crates/talksage-plugins/src/webhook.rs` | webhook 推送 finalizer |
| 修改 `crates/talksage-plugins/src/builtin.rs` | 注册两个 finalizer + 顺序不变量测试 |
| 修改 `crates/talksage-pipeline/src/service.rs` | `finish()` 改走 finalizer 链；实现并注入 `QualityDeps` / `WebhookDeps` |

---

## Task 1: `SessionFinalizer` trait 与 `FinalizeContext`

**Files:**
- Modify: `crates/talksage-plugins/src/registry.rs`

- [ ] **Step 1: 写失败的测试**

在 `registry.rs` 的测试区追加：

```rust
#[cfg(test)]
mod finalizer_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Recording(&'static str, Arc<Mutex<Vec<&'static str>>>);
    impl SessionFinalizer for Recording {
        fn name(&self) -> &'static str { self.0 }
        fn finalize(&self, _ctx: &FinalizeContext) -> anyhow::Result<()> {
            self.1.lock().unwrap().push(self.0);
            Ok(())
        }
    }

    struct Failing(Arc<AtomicUsize>);
    impl SessionFinalizer for Failing {
        fn name(&self) -> &'static str { "failing" }
        fn finalize(&self, _ctx: &FinalizeContext) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            anyhow::bail!("故意失败")
        }
    }

    fn ctx() -> FinalizeContext<'static> {
        FinalizeContext { session_id: 1, quality: None }
    }

    #[test]
    fn finalizers_run_in_registration_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut hooks = HookRegistry::default();
        hooks.add_finalizer(Arc::new(Recording("first", log.clone())));
        hooks.add_finalizer(Arc::new(Recording("second", log.clone())));
        hooks.run_finalizers(&ctx());
        assert_eq!(*log.lock().unwrap(), vec!["first", "second"]);
    }

    /// 关键契约：一个 finalizer 失败不得阻塞后续的。
    /// webhook 打不通，不能因此丢掉质量评估的写库。
    #[test]
    fn a_failing_finalizer_does_not_block_the_rest() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut hooks = HookRegistry::default();
        hooks.add_finalizer(Arc::new(Failing(calls.clone())));
        hooks.add_finalizer(Arc::new(Recording("after", log.clone())));
        let report = hooks.run_finalizers(&ctx());
        assert_eq!(calls.load(Ordering::Relaxed), 1, "失败的那个应被调用过");
        assert_eq!(*log.lock().unwrap(), vec!["after"], "后续 finalizer 必须照常执行");
        assert_eq!(report.failed, vec!["failing"], "失败项应汇总上报");
    }

    #[test]
    fn empty_registry_reports_no_failures() {
        let hooks = HookRegistry::default();
        assert!(hooks.run_finalizers(&ctx()).failed.is_empty());
    }
}
```

- [ ] **Step 2: 运行，确认失败**

```bash
cd /Users/robot/projects/talk-sage && source .env-worktree
cargo test -p talksage-plugins --lib finalizer 2>&1 | grep -E "^error" | head -5
```

预期：`cannot find trait SessionFinalizer` / `cannot find type FinalizeContext`。

- [ ] **Step 3: 实现**

在 `registry.rs` 中 `SegmentObserver` 之后插入：

```rust
/// finalizer 的输入。会话已停、已落库，此处只读。
///
/// 刻意保持极简：finalizer 需要的持久数据都能用 `session_id` 从 SessionStore
/// 查到，把整个 store 塞进 context 会让插件能改库，破坏「会后只读」的约束。
pub struct FinalizeContext<'a> {
    pub session_id: i64,
    /// 由 `session_quality` 写入，供后续 finalizer 读取（如 webhook 载荷）。
    /// 因此 `session_quality` 必须排在链首。
    pub quality: Option<&'a str>,
}

/// 会后钩子：`stop → flush → 落库` 之后执行，不占实时路径。
pub trait SessionFinalizer: Send + Sync {
    fn name(&self) -> &'static str;
    /// 返回 Err 只记录并继续下一个 —— 逐个独立，互不阻塞。
    fn finalize(&self, ctx: &FinalizeContext) -> anyhow::Result<()>;
}

/// `run_finalizers` 的结果汇总。
#[derive(Debug, Default)]
pub struct FinalizeReport {
    /// 执行失败的 finalizer 名字。
    pub failed: Vec<&'static str>,
}
```

在 `HookRegistry` 中加字段与方法：

```rust
    finalizers: Vec<Arc<dyn SessionFinalizer>>,
```

```rust
    pub fn add_finalizer(&mut self, f: Arc<dyn SessionFinalizer>) {
        self.finalizers.push(f);
    }

    pub fn finalizer_count(&self) -> usize {
        self.finalizers.len()
    }

    /// 依次执行，逐个独立：任一失败只记录并继续，不中断链条。
    pub fn run_finalizers(&self, ctx: &FinalizeContext) -> FinalizeReport {
        let mut report = FinalizeReport::default();
        for f in &self.finalizers {
            if let Err(e) = f.finalize(ctx) {
                log::warn!("finalizer[{}] 失败: {e}", f.name());
                report.failed.push(f.name());
            }
        }
        report
    }
```

`registry.rs` 顶部需要 `use std::sync::Mutex;`（测试用）。

- [ ] **Step 4: 运行确认通过**

```bash
cargo test -p talksage-plugins --lib finalizer
```

预期：`3 passed`。

- [ ] **Step 5: 全量 + golden 不变，然后提交**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
git diff --exit-code crates/talksage-pipeline/tests/golden/ && echo "golden 未变 ✓"
git add crates/talksage-plugins/src/registry.rs
git commit -m "feat(plugins): SessionFinalizer 钩子

会后钩子，stop→flush→落库之后执行。逐个独立：任一失败只记录并继续，
webhook 打不通不能因此丢掉质量评估的写库。"
```

---

## Task 2: `session_quality` finalizer

**Files:**
- Create: `crates/talksage-plugins/src/session_quality.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`

现状：`service.rs` 的 `finish()` 里内联做 `SessionMeta::evaluate` + `set_session_meta`。它需要 `SessionStore` 与 `QualityParams`，而 `FinalizeContext` 刻意不带 store。

**因此本插件采用「构造时注入依赖」**：`SessionQualityPlugin` 在 `register()` 时把 store 与配置快照捕获进 finalizer 实例。这与 `SegmentObserver` 用 `PluginContext` 注入是同一思路。

- [ ] **Step 1: 写失败的测试**

创建 `crates/talksage-plugins/src/session_quality.rs`，先只写测试：

```rust
//! session_quality：会话结束时评估质量并写入 sessions.meta。
//!
//! 迁移自 service.rs 的 finish()。必须排在 finalizer 链首位——
//! 它写入 FinalizeContext.quality，webhook 载荷要带这个结论。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_one_finalizer() {
        let p = SessionQualityPlugin;
        assert_eq!(p.id(), "session_quality");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &mut hooks);
        assert_eq!(hooks.finalizer_count(), 1);
        assert_eq!(hooks.observers().len(), 0, "本插件不挂 observer");
    }

    #[test]
    fn default_config_has_enabled() {
        assert!(SessionQualityPlugin.default_config().enabled());
    }
}
```

> 质量判定逻辑本身（`SessionMeta::evaluate`）已在 `talksage-session` 中有测试覆盖，本插件只负责调度，因此这里只测注册行为。搬迁的正确性由 Task 4 的集成验证保证。

- [ ] **Step 2: 挂模块并运行确认失败**

`lib.rs` 加 `pub mod session_quality;`，然后：

```bash
cargo test -p talksage-plugins --lib session_quality 2>&1 | grep -E "^error" | head -3
```

预期：`cannot find type SessionQualityPlugin`。

- [ ] **Step 3: 实现**

```rust
use std::sync::Arc;

use serde_json::json;

use crate::registry::{FinalizeContext, HookRegistry, Plugin, PluginConfig, SessionFinalizer};

/// 依赖在构造时注入 —— FinalizeContext 刻意不带 SessionStore，
/// 避免插件通过 context 改库，破坏「会后只读」。
pub struct SessionQualityFinalizer {
    pub deps: Option<Arc<dyn QualityDeps>>,
}

/// 质量评估所需的宿主能力。由 pipeline 侧实现并注入。
pub trait QualityDeps: Send + Sync {
    /// 评估并写入 meta，返回质量标签（如 "clean" / "noise"）。
    fn evaluate_and_store(&self, session_id: i64) -> anyhow::Result<String>;
}

impl SessionFinalizer for SessionQualityFinalizer {
    fn name(&self) -> &'static str {
        "session_quality"
    }

    fn finalize(&self, ctx: &FinalizeContext) -> anyhow::Result<()> {
        let Some(deps) = &self.deps else {
            return Ok(()); // 无依赖（如单测）时静默跳过
        };
        let label = deps.evaluate_and_store(ctx.session_id)?;
        log::info!("会话 #{} 质量评估: {label}", ctx.session_id);
        Ok(())
    }
}

pub struct SessionQualityPlugin;

impl Plugin for SessionQualityPlugin {
    fn id(&self) -> &'static str {
        "session_quality"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true }))
    }

    fn register(&self, _cfg: &PluginConfig, hooks: &mut HookRegistry) {
        hooks.add_finalizer(Arc::new(SessionQualityFinalizer { deps: None }));
    }
}
```

> **注意**：`register()` 建的实例 `deps: None`。真实依赖由 `service.rs` 在构建注册表后注入 —— 见 Task 4。这是本阶段唯一的妥协：`Plugin::register` 拿不到宿主依赖。若 Task 4 实施时发现注入很别扭，停下来报告，可能需要给 `register` 加一个宿主上下文参数。

- [ ] **Step 4: 运行确认通过并提交**

```bash
cargo test -p talksage-plugins --lib session_quality
git add crates/talksage-plugins/src/session_quality.rs crates/talksage-plugins/src/lib.rs
git commit -m "feat(plugins): session_quality finalizer"
```

---

## Task 3: `webhook` finalizer

**Files:**
- Create: `crates/talksage-plugins/src/webhook.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`

结构与 Task 2 同：trait `WebhookDeps { fn push(&self, session_id: i64, quality: Option<&str>) -> anyhow::Result<()> }`，插件注册一个 finalizer，依赖由 `service.rs` 注入。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_one_finalizer() {
        let p = WebhookPlugin;
        assert_eq!(p.id(), "webhook");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &mut hooks);
        assert_eq!(hooks.finalizer_count(), 1);
    }

    /// webhook 默认关闭 —— 它会把会话内容发到外部地址，
    /// 不能因为「新增了插件」就默认开启。
    #[test]
    fn webhook_is_disabled_by_default() {
        assert!(!WebhookPlugin.default_config().enabled());
    }
}
```

- [ ] **Step 2–4**: 同 Task 2 的节奏（挂模块 → 确认失败 → 实现 → 通过 → 提交）。实现中 `default_config` 用 `json!({ "enabled": false })`。

提交信息：

```
feat(plugins): webhook finalizer

默认关闭：它把会话内容发到外部地址，不能因为「新增了插件」就默认开启。
实际启用仍由 [webhooks] 配置决定（见 Task 4 的注入）。
```

---

## Task 4: 接进 `finish()`，删除旧的内联调度

**Files:**
- Modify: `crates/talksage-plugins/src/builtin.rs`
- Modify: `crates/talksage-pipeline/src/service.rs`

- [ ] **Step 1: 注册进中心表 + 顺序不变量测试**

`builtin.rs` 追加两个插件，并加测试：

```rust
    /// 设计 §3.4 S2：session_quality 必须在 webhook 之前 ——
    /// 它写入 FinalizeContext.quality，webhook 载荷要带这个结论。
    #[test]
    fn session_quality_is_ordered_before_webhook() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        let q = ids.iter().position(|id| *id == "session_quality").expect("缺少 session_quality");
        let w = ids.iter().position(|id| *id == "webhook").expect("缺少 webhook");
        assert!(q < w, "session_quality 必须排在 webhook 之前，实际: {ids:?}");
    }
```

- [ ] **Step 2: 在 service.rs 实现并注入依赖**

在 `service.rs` 中为 `TalkSageService` 实现 `QualityDeps` 与 `WebhookDeps`（把现有 `finish()` 里的逻辑原样搬过去），并在 `build_live_config` 构建注册表后注入。

**若注入方式别扭到需要改 `Plugin::register` 签名，停下来报告，不要硬凑。**

- [ ] **Step 3: `finish()` 改走 finalizer 链**

删除 `finish()` 中的质量评估块与 webhook 块，替换为：

```rust
        let ctx = talksage_plugins::FinalizeContext { session_id: sid, quality: None };
        let report = hooks.run_finalizers(&ctx);
        if !report.failed.is_empty() {
            log::warn!("会话 #{sid} 收尾有 {} 项失败: {:?}", report.failed.len(), report.failed);
        }
```

- [ ] **Step 4: 验证**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
git diff --exit-code crates/talksage-pipeline/tests/golden/ && echo "golden 未变 ✓"
```

**golden 必须不变** —— 质量评估与 webhook 都发生在管道停止之后，不产生 `DomainEvent`，不应影响事件序列。若 golden 变了，说明搬迁改了实时路径，排查后再继续。

手工验证 webhook 仍受配置控制：确认 `[webhooks] enabled = false` 时 finalizer 不发请求。

- [ ] **Step 5: 提交**

```bash
git add crates/talksage-plugins/src/builtin.rs crates/talksage-pipeline/src/service.rs
git commit -m "refactor(service): 会话收尾改走 finalizer 链

质量评估与 webhook 从 finish() 的内联代码搬到 finalizer。逐个独立，
webhook 失败不再有机会影响质量评估的写库。golden 不变——两者都在
管道停止之后，不产生 DomainEvent。"
```

---

## ~~Task 5: 导出与纪要去重~~（已取消）

**取消原因：前提是错的，核对代码后不存在重复实现。**

写计划时我按设计 §3.5 的说法，认为 server 与 tauri 各有一份导出实现。实际是：

```rust
// tauri:  get_session → talksage_session::export_markdown(&detail) → 写文件 + 返回 {path, content}
// server: get_session → talksage_session::export_markdown(&detail) → 返回 text/markdown body
```

**实现只有一份**（`talksage_session::export_markdown`），两个适配器是薄封装，差异
是真实的产品差异：桌面端额外落文件到 `<data_dir>/exports/`，服务端直接返回响应体。
这已经是正确设计。

trio 纪要同理：两边都调 `talksage_notes::TrioGenerator::generate`，仅有约 6 行编排
代码（`build_llm` → `get_session` → `generate` → `set_trio`）形似。对适配器而言这个
程度的重复可以接受，抽取收益不抵新增一层间接的成本。

**结论：阶段 4 只做 finalizer 钩子（Task 1–4）。** 若将来编排代码继续膨胀，再考虑
抽 `TalkSageService` 方法。

## Task 6: 收尾核对

- [ ] **Step 1: 结构核对**

```bash
grep -c "SessionMeta::evaluate\|trigger_meeting_webhooks" crates/talksage-pipeline/src/service.rs   # 应为 0（已搬进插件依赖实现）或仅在 Deps impl 内
wc -l crates/talksage-pipeline/src/service.rs                                                        # 阶段 3 末为基准，应下降
```

- [ ] **Step 2: 全量验证**

```bash
export TALKSAGE_REQUIRE_MODELS=1
cargo test --workspace 2>&1 | grep -cE "test result: ok"
(cd web && npx vitest run 2>&1 | grep -E "Tests +[0-9]")
cargo clippy --workspace --all-targets 2>&1 | grep -E "finalizer|session_quality|webhook" | head
```

- [ ] **Step 3: 更新设计文档**

§7 阶段表标记阶段 4 完成；记录实际的 finalizer 数量（2 个，非 4 个）与 `service.rs` 行数变化。

---

## 阶段 5 的入口

阶段 4 完成后，仅剩配置层：`[plugins.<id>]` 通用表、场景 allowlist、`GET /plugins` 元数据端点、设置 UI 自动生成。届时 `service.rs` 中 term/translator/brief 的手工装配也应收敛到注册表，`cfg.plugins` 字段可以删除，`make_on_final` 的 `chain` 退回单一来源。
