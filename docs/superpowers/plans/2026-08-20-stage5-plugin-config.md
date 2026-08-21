# 阶段 5：插件配置通用化与设置 UI 自动生成

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把最后三个手工装配的分析插件收进注册表，用通用 `[plugins.<id>]` 表取代 `PluginsConfig` 的具名字段，新增 `/plugins` 元数据端点，设置 UI 改为按元数据生成。

**Architecture:** `builtin_plugins()` 成为唯一插件来源，`LivePipelineConfig.plugins` 字段删除，`make_on_final` 退回单一来源。场景对插件的门控从「if 链里写死的布尔」变成「场景提供一份 allowlist」。

**Tech Stack:** Rust 2021 + React/TS，不新增依赖、不新增 crate。

**对应设计：** [2026-08-20-everything-is-a-plugin-design.md](../specs/2026-08-20-everything-is-a-plugin-design.md) 阶段 5。前置：阶段 1–4 已合并（`c4f62c4`）。

---

## 前提核对（已于 2026-08-20 对照代码确认）

| 前提 | 现状 |
|---|---|
| 注册表已有 5 个插件 | `short_segment` / `cross_stream_dedup` / `conversation_metrics` / `session_quality` / `webhook` |
| term / translator / brief **不在注册表** | 仍由 `service.rs:367-380` 手工 `plugins.push(...)` |
| 场景门控 | `scene.X_enabled && snapshot.plugins.X.enabled` 两道与门 |
| `brief_retriever` 还有第三道门 | `&& kb.is_some()` —— 知识库未索引时不装配 |
| `/plugins` 端点 | 不存在 |
| 设置 UI | `SettingsSection.tsx:57-59, 190-192` 三个插件各一组硬编码状态与提交字段 |

---

## 文件结构

| 文件 | 职责 |
|---|---|
| 修改 `crates/talksage-plugins/src/{term_explainer,translator,brief_retriever}.rs` | 各加一个 `Plugin` 实现 |
| 修改 `crates/talksage-plugins/src/builtin.rs` | 三者进清单；新增 `plugin_metadata()` 供 UI 使用 |
| 修改 `crates/talksage-config/src/lib.rs` | `PluginsConfig` 具名字段 → `BTreeMap<String, Value>`；`SceneParams` 的三个布尔 → `allowlist` |
| 修改 `crates/talksage-pipeline/src/service.rs` | 删手工装配；场景 allowlist 并入 overrides |
| 修改 `crates/talksage-pipeline/src/lib.rs` | 删 `LivePipelineConfig.plugins`；`make_on_final` 单一来源 |
| 修改 `crates/talksage-server/src/lib.rs` | 新增 `GET /plugins` |
| 修改 `web/src-tauri/src/lib.rs` | 新增 `list_plugins` command |
| 修改 `web/src/sections/SettingsSection.tsx` | 按元数据生成表单 |
| 修改 `web/src/lib/api.ts` | `/plugins` 类型与调用 |

---

## Task 1: 三个分析插件进注册表

**Files:** `crates/talksage-plugins/src/{term_explainer,translator,brief_retriever,builtin}.rs`

- [ ] **Step 1: 写失败的测试**

在 `builtin.rs` 测试区追加：

```rust
    /// 阶段 5：三个分析插件必须进注册表，service.rs 不再手工装配。
    #[test]
    fn analysis_plugins_are_in_the_registry() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        for want in ["term_explainer", "translator", "brief_retriever"] {
            assert!(ids.contains(&want), "缺少插件 {want}，实际: {ids:?}");
        }
    }

    /// brief_retriever 依赖知识库：ctx.kb 为 None 时不应注册 observer。
    /// 这是它相对其他插件多出来的一道门（原 service.rs 的 `&& kb.is_some()`）。
    #[test]
    fn brief_retriever_needs_a_knowledge_base() {
        let p = crate::brief_retriever::BriefRetrieverPluginDef;
        let mut without = HookRegistry::default();
        p.register(&p.default_config(), &PluginContext::new(), &mut without);
        assert_eq!(without.observers().len(), 0, "无知识库时不应注册");

        let mut kb = talksage_knowledge::KnowledgeBase::new();
        let _ = kb.index_folder(std::path::Path::new("."));
        let ctx = PluginContext { kb: Some(Arc::new(kb)), ..PluginContext::new() };
        let mut with = HookRegistry::default();
        p.register(&p.default_config(), &ctx, &mut with);
        assert_eq!(with.observers().len(), 1, "有知识库时应注册");
    }
```

- [ ] **Step 2: 运行确认失败**

```bash
cd /Users/robot/projects/talk-sage && source .env-worktree
cargo test -p talksage-plugins --lib builtin 2>&1 | grep -E "^error|FAILED" | head -5
```

预期：`analysis_plugins_are_in_the_registry` 失败（缺三个 id）；`BriefRetrieverPluginDef` 类型不存在。

- [ ] **Step 3: 实现**

三个文件各加一个 `Plugin` 实现。命名用 `<Name>PluginDef` 以免与既有的 `<Name>Plugin`（observer 本体）撞名。

`term_explainer.rs`：
```rust
pub struct TermExplainerPluginDef;

impl crate::registry::Plugin for TermExplainerPluginDef {
    fn id(&self) -> &'static str { "term_explainer" }
    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(serde_json::json!({ "enabled": true, "cooldown_seconds": 10.0 }))
    }
    fn register(&self, cfg: &PluginConfig, _ctx: &PluginContext, hooks: &mut HookRegistry) {
        hooks.add_observer(Arc::new(TermExplainerPlugin::new(
            cfg.get_f64("cooldown_seconds", 10.0),
        )));
    }
}
```

`translator.rs` 同理（无额外配置项）。

`brief_retriever.rs` 多一道知识库门：
```rust
    fn register(&self, cfg: &PluginConfig, ctx: &PluginContext, hooks: &mut HookRegistry) {
        // 原 service.rs 的 `&& kb.is_some()`：知识库没索引到内容时不装配，
        // 否则每段都会白跑一次检索。
        if ctx.kb.is_none() {
            return;
        }
        hooks.add_observer(Arc::new(BriefRetrieverPlugin::new(
            cfg.get_f64("cooldown_seconds", 15.0),
            cfg.get_f64("min_score", 0.05) as f32,
        )));
    }
```

`builtin.rs` 的 `builtin_plugins()` 在 `ConversationMetricsPlugin` 之后、finalizer 之前插入三者。

> **默认值必须与现状逐字一致**（已对照 `crates/talksage-config/src/lib.rs:666-684` 与 `service.rs:369-379` 核实）：
>
> | 插件 | `enabled` | `cooldown_seconds` | 其他 |
> |---|---|---|---|
> | `term_explainer` | true | **10.0** | — |
> | `translator` | true | **3.0** | 见下方说明 |
> | `brief_retriever` | true | **15.0** | `min_score` 0.05（硬编码在 `service.rs` 调用点，不在配置里） |
>
> **`translator` 有个坑**：配置里有 `cooldown_seconds: 3.0`，但 `TranslatorPlugin::new()` **不接受任何参数** —— 这个值当前根本没被使用。迁移时保持现状：`default_config()` 里保留 `cooldown_seconds: 3.0` 以免用户配置突然消失，但 `register()` 不读它。**不要顺手"修复"成传进去** —— 那会改变翻译插件的触发频率，属于行为变更，应单独提出。

- [ ] **Step 4: 通过 + 全量 + golden 不变，提交**

```bash
cargo test -p talksage-plugins --lib builtin
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
git diff --exit-code crates/talksage-pipeline/tests/golden/ && echo "golden 未变 ✓"
```

此时三个插件在注册表里**同时**仍被 `service.rs` 手工装配 —— 会重复派发。Task 2 删掉手工那份。**本步不要跑真实会话验证**，等 Task 2 完成再看。

```bash
git commit -m "feat(plugins): 三个分析插件进注册表

term/translator/brief 此前由 service.rs 手工装配。brief 的知识库门
（原 && kb.is_some()）移入 register()，由 PluginContext.kb 判断。"
```

---

## Task 2: 删除手工装配，`cfg.plugins` 字段下线

**Files:** `crates/talksage-pipeline/src/{service.rs,lib.rs}`、`crates/talksage-pipeline/tests/*.rs`

- [ ] **Step 1: 删 service.rs 的手工装配**

删除 `service.rs` 中 `let mut plugins: Vec<Arc<dyn SegmentObserver>> = Vec::new();` 到三个 `plugins.push(...)` 结束的整块，以及 `LivePipelineConfig` 构造里的 `plugins` 字段。

场景门控暂时保留原样（用 overrides 表达），Task 3 再换成 allowlist：

```rust
        // 场景关掉的插件：用 enabled=false 覆盖（Task 3 换成 allowlist）
        if !scene.term_enabled {
            plugin_overrides.insert("term_explainer".into(), serde_json::json!({ "enabled": false }));
        }
        if !scene.translation_enabled {
            plugin_overrides.insert("translator".into(), serde_json::json!({ "enabled": false }));
        }
        if !scene.brief_enabled {
            plugin_overrides.insert("brief_retriever".into(), serde_json::json!({ "enabled": false }));
        }
```

同时把用户配置并进 overrides（`snapshot.plugins.term_explainer.enabled` 等）。

- [ ] **Step 2: 删 `LivePipelineConfig.plugins` 字段**

`lib.rs` 中删除该字段，`make_on_final` 的遍历退回单一来源：

```rust
        for plugin in cfg.hooks.observers() {
```

（阶段 3 引入的 `cfg.plugins.iter().chain(...)` 到此结束。）

- [ ] **Step 3: 修所有构造点**

`offline.rs`、`characterization.rs`、`pipeline_live.rs` 里所有 `plugins: Vec::new()` / `plugins: vec![...]` 删除。测试若需要注入自定义 observer，改为构造带该 observer 的 `HookRegistry`。

`pipeline_live.rs` 里 `plugins_emit_term_and_translation_events` 与 `filtered_segments_never_reach_observers` 两个测试依赖注入观察者，注意改造后仍要能注入。

- [ ] **Step 4: 验证**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
git diff --exit-code crates/talksage-pipeline/tests/golden/
```

**golden 必须不变。** 若变了，最可能是插件被重复派发（Task 1 的注册表 + 遗留的手工装配都还在）或某个插件的默认值与原配置不符。

特别确认 `plugins_emit_term_and_translation_events` 仍绿 —— 它验证术语与翻译插件真的产出事件。

- [ ] **Step 5: 提交**

```bash
git commit -m "refactor(pipeline): 删除手工装配的插件列表

builtin_plugins() 成为唯一插件来源。LivePipelineConfig.plugins 字段
下线，make_on_final 退回单一来源（阶段 3 引入的 chain 到此结束）。"
```

---

## Task 3: 配置换通用表 + 场景 allowlist

**Files:** `crates/talksage-config/src/lib.rs`、`crates/talksage-pipeline/src/service.rs`

- [ ] **Step 1: 写失败的测试**

在 `talksage-config` 的测试区：

```rust
    #[test]
    fn plugins_config_is_a_generic_table() {
        let mut c = Config::default();
        c.plugins.entries.insert(
            "term_explainer".into(),
            serde_json::json!({ "enabled": false, "cooldown_seconds": 99.0 }),
        );
        let toml = toml::to_string(&c).expect("应可序列化");
        assert!(toml.contains("term_explainer"), "通用表应写进 toml");
    }

    /// 场景用 allowlist：不在列表里的插件一律关闭。
    /// 用 allowlist 而非 denylist —— 新增插件不会因为某个场景忘了更新而意外开启。
    #[test]
    fn life_scene_allows_no_analysis_plugins() {
        let allow = scene_params(SceneMode::Life).plugin_allowlist;
        for id in ["term_explainer", "translator", "brief_retriever"] {
            assert!(!allow.contains(&id.to_string()), "生活模式不应允许 {id}");
        }
    }

    #[test]
    fn meeting_scene_allows_all_analysis_plugins() {
        let allow = scene_params(SceneMode::Meeting).plugin_allowlist;
        for id in ["term_explainer", "translator", "brief_retriever"] {
            assert!(allow.contains(&id.to_string()), "会议模式应允许 {id}");
        }
    }
```

- [ ] **Step 2: 确认失败，然后实现**

`PluginsConfig` 改为：
```rust
pub struct PluginsConfig {
    /// 通用插件表：键是插件 id，值由插件自己的 default_config() 定义结构。
    #[serde(flatten)]
    pub entries: std::collections::BTreeMap<String, serde_json::Value>,
    pub notes: NotesConfig,
}
```

`SceneParams` 的 `term_enabled` / `translation_enabled` / `brief_enabled` 三个布尔替换为：
```rust
    /// 该场景允许启用的插件 id。不在列表里的一律关闭。
    pub plugin_allowlist: Vec<String>,
```

`scene_params()` 里三个场景各给出列表（生活模式为空或仅含 filter 类）。

> **破坏性变更（设计 §4 已决策）**：旧的具名字段不做读时迁移，用户 `talksage.toml` 里的三个开关与 cooldown 回落到插件默认值。会议场景默认全开，实际影响小。

- [ ] **Step 3: service.rs 用 allowlist 裁决**

```rust
        // 合并顺序：plugin.default_config() → 用户 [plugins.<id>] → 场景 allowlist 最后裁决
        let mut plugin_overrides = snapshot.plugins.entries.clone();
        for p in talksage_plugins::builtin_plugins() {
            let id = p.id();
            // 分析类插件受场景 allowlist 约束；filter/finalizer 类不受（它们是基础设施）
            if ANALYSIS_PLUGIN_IDS.contains(&id) && !scene.plugin_allowlist.iter().any(|a| a == id) {
                plugin_overrides
                    .entry(id.to_string())
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
                    .map(|o| o.insert("enabled".into(), serde_json::Value::Bool(false)));
            }
        }
```

`ANALYSIS_PLUGIN_IDS` 定义在 `talksage-plugins`，只含三个分析插件。

> **为什么 filter/finalizer 不受 allowlist 约束**：短段抑制、跨流去重、质量评估是基础设施，不是「会议辅助功能」。生活模式关掉术语解释是产品意图；关掉短段抑制不是。

- [ ] **Step 4: 验证并提交**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED|failures:"
git diff --exit-code crates/talksage-pipeline/tests/golden/
```

手工验证三个场景：切到生活模式确认分析插件不触发、会议模式确认触发。

---

## Task 4: `/plugins` 元数据端点

**Files:** `crates/talksage-plugins/src/builtin.rs`、`crates/talksage-server/src/lib.rs`、`web/src-tauri/src/lib.rs`

- [ ] **Step 1: 元数据函数 + 测试**

`builtin.rs`：
```rust
/// 供设置 UI 生成表单：每个插件的 id 与默认配置。
pub fn plugin_metadata() -> Vec<serde_json::Value> {
    builtin_plugins()
        .iter()
        .map(|p| serde_json::json!({ "id": p.id(), "schema": p.default_config().as_value() }))
        .collect()
}
```

测试：条目数等于 `builtin_plugins().len()`，每条都有 `id` 与 `schema.enabled`。

- [ ] **Step 2: server 端点**

`GET /plugins`，走 `token_ok` 鉴权（与其他端点一致），返回 `plugin_metadata()`。

- [ ] **Step 3: Tauri command**

`list_plugins` 返回同一结构，注册进 `invoke_handler`。

- [ ] **Step 4: 验证**

```bash
cargo test --workspace 2>&1 | grep -E "^error|FAILED"
# 手工：起 server 后
curl -s http://127.0.0.1:8080/api/plugins | head -c 400
```

预期返回 8 个插件（5 + 3 分析类）的 id 与默认配置。

---

## Task 5: 设置 UI 按元数据生成

**Files:** `web/src/lib/api.ts`、`web/src/sections/SettingsSection.tsx`

- [ ] **Step 1: api.ts 加类型与调用**

```ts
export interface PluginMeta { id: string; schema: Record<string, unknown> }
export async function listPlugins(): Promise<PluginMeta[]> {
  // 双载体分支：照 api.ts 里其他函数的既有写法（Tauri 用 invoke，
  // 浏览器用 fetch + token 头）。实施时打开 api.ts 对齐现有模式，
  // 不要新造一种调用风格。
  ...
}
```

- [ ] **Step 2: SettingsSection 改造**

删除 `termEnabled` / `transEnabled` / `briefEnabled` 三组 state（`SettingsSection.tsx:57-59`）与提交时的三个具名字段（`:190-192`），改为：

- 启动时 `listPlugins()` 拉元数据
- 按 `schema` 的键类型渲染控件：`bool` → 开关，`number` → 数字输入，`string` → 文本框
- 提交时组装成 `{ plugins: { <id>: {...} } }`

- [ ] **Step 3: 前端测试**

`web/src/lib/` 下加一个纯函数测试：给定元数据与用户值，产出正确的提交载荷。UI 渲染不测（现有前端测试都只覆盖 lib 层纯函数，遵循同一惯例）。

- [ ] **Step 4: 验证**

```bash
cd web && npx tsc --noEmit && npx vitest run
```

手工：打开设置页，确认三个分析插件的开关仍在、可切换、保存后生效。**这一步必须真机确认** —— 前端测试不覆盖渲染。

---

## Task 6: 收尾

- [ ] **Step 1: 结构核对**

```bash
grep -c "TermExplainerPlugin::new\|TranslatorPlugin::new\|BriefRetrieverPlugin::new" crates/talksage-pipeline/src/service.rs   # 应为 0
grep -c "cfg.plugins\|pub plugins:" crates/talksage-pipeline/src/lib.rs                                                        # 应为 0
grep -c "term_explainer\|translator\|brief_retriever" web/src/sections/SettingsSection.tsx                                      # 应为 0
wc -l crates/talksage-pipeline/src/service.rs                                                                                   # 基线见下
```

- [ ] **Step 2: 全量**

```bash
export TALKSAGE_REQUIRE_MODELS=1
cargo test --workspace 2>&1 | grep -cE "test result: ok"
(cd web && npx vitest run 2>&1 | grep -E "Tests +[0-9]")
cargo clippy --workspace --all-targets 2>&1 | grep -E "^warning" | wc -l
```

- [ ] **Step 3: 加一个插件的成本验证（本次重构的最终验收）**

人工核对：新增一个 observer 类插件现在需要改几处？应为 **2 处**（`plugins/` 下加文件 + `builtin.rs` 加一行）。不需要改 pipeline、不需要改 config 结构体、不需要改前端。

若不满足，说明接缝没做干净 —— 这是整个五阶段重构的最终验收标准，比任何行数指标都重要。

- [ ] **Step 4: 更新设计文档**

§7 阶段表标记阶段 5 完成；补一节「最终状态」记录插件总数、加插件成本、以及五个阶段各自对 `pipeline/lib.rs` 行数的实际影响（含阶段 1–2 的净增 35 行，作为「搬判定逻辑不瘦身」的反例）。

---

## 完成后

五个阶段全部落地，`talksage-plugins` 成为唯一的插件来源。仍未做、也明确不做的：第三方插件 / 动态加载（Rust 无稳定 ABI）、完整 Cordis 化。若将来需要第三方生态，唯一靠谱路径是 WASM 宿主（`extism` / `wasmtime`），那是独立产品项。

遗留的两条已知债，见设计文档：`PluginContext` 每加一个宿主能力就多一个 `Option` 字段（O(N)）；webhook finalizer 返回 `Ok` 只代表「已派发」而非「已送达」。
