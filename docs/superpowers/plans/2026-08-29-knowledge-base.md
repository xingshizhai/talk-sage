# 知识库（源插件 + 底盘 + 三消费者）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地一期知识库：一个 Obsidian vault 作为源插件，统一 retrieve，供会中材料包、纪要/智能纪要、AI 助手使用。

**Architecture:** `KnowledgeSource` 只负责读盘切块；`KnowledgeIndex`（现 `KnowledgeBase` 演进）负责索引与检索；`KnowledgeHub` 由配置驱动刷新并给 pipeline / chat / 纪要入口共用。会中 observer 不再 `index_folder`。

**Tech Stack:** Rust workspace（`talksage-knowledge` / `plugins` / `config` / `pipeline` / `notes` / Tauri / React）。词法检索沿用现实现，不加向量。

**Spec:** [2026-08-29-knowledge-base-design.md](../specs/2026-08-29-knowledge-base-design.md)

**提交：** 每组任务结束后，仅在用户明确要求时 `git commit`。不要把 `config/talksage.toml` 或密钥写入版本库。

---

## 文件职责

| 路径 | 职责 |
|---|---|
| `crates/talksage-knowledge/src/lib.rs` | `KnowledgeSnippet`、`KnowledgeSource`、`ObsidianSource`、`KnowledgeHit.source_id`、`list_documents`、`format_knowledge_block` |
| `crates/talksage-plugins/src/knowledge_obsidian.rs` | 源插件身份与配置；`register` 为空 |
| `crates/talksage-plugins/src/registry.rs` | `PluginCategory::KnowledgeSource`、`PluginPhase::Source` |
| `crates/talksage-plugins/src/builtin.rs` | 清单增加源插件（不进 analysis allowlist） |
| `crates/talksage-plugins/src/brief_retriever.rs` | 默认 `enabled: false` |
| `crates/talksage-config/src/lib.rs` | 旧 `[knowledge_base]` 迁入 `plugins.knowledge_obsidian` |
| `crates/talksage-pipeline/src/knowledge.rs`（新建） | `KnowledgeHub`：刷新、retrieve、文档列表 |
| `crates/talksage-pipeline/src/service.rs` | 用 Hub 注入 `PluginContext.kb`；去掉自行 `index_folder` |
| `crates/talksage-pipeline/src/chat.rs` | 有命中则追加到 system prompt |
| `crates/talksage-notes/prompts/*` | `{knowledge}` 与引用约束 |
| `crates/talksage-notes/src/lib.rs` | `generate` / trio 增加 `knowledge` 参数 |
| `crates/talksage-session/src/lib.rs` | `SessionMeta.pinned_note_paths` |
| `web/src-tauri/src/lib.rs` | `list_knowledge_documents`；`start_listen` 传钉住路径 |
| `web/src/sections/SettingsSection.tsx` | 文案与绑定改到源插件 |
| `web/src/App.tsx`、`AsidePanel.tsx` | 材料包选择与右栏展示 |
| `config/talksage.example.toml`、`docs/architecture-v2.md`、`docs/plugin-development.md`、README | 与实现同步 |

不要改 `generate_highlights`。不要加第二种源或向量。

---

### Task 1: 片段、Obsidian 源、命中带来源 id

**Files:**
- Modify: `crates/talksage-knowledge/src/lib.rs`
- Test: 同文件 `#[cfg(test)]`

- [ ] **Step 1: 写失败测试** — 临时目录建 vault：`.obsidian/app.json`、`.trash/x.md`、`.git/ignored.md`、`200-Wiki/npi.md`（含标题与「样品交期」）。断言 `ObsidianSource::load_snippets` 只得到 wiki 那篇；`path` 为相对路径；`source_id == "knowledge_obsidian"`。再断言不存在的根路径返回 `Err`。

- [ ] **Step 2: 跑测试，确认失败**（类型还不存在）。

```text
cargo test -p talksage-knowledge --lib
```

- [ ] **Step 3: 实现** `KnowledgeSnippet`、`trait KnowledgeSource`、`ObsidianSource`。把现有 `walk_files` / `chunk_file` 接到 `load_snippets`：每块带 `source_id` 与相对 `path`。`KBHit`/`KnowledgeHit` 增加 `source_id: String`（旧测试里 `source` 仍是路径）。`rebuild`/`index_snippets`：用片段列表重建现有 token/IDF，替换「只从 folder 索引」为底层 API；`index_folder` 可改为 `ObsidianSource` + rebuild 的薄封装，以免一步拆光调用方。

- [ ] **Step 4: 增加** `list_documents()`（按 path 去重，title = 第一标题或文件名）和 `format_knowledge_block(hits) -> String`（无命中返回空字符串）。测试：两块同文件 → 文档列表一条；有 hit 的 block 含 `knowledge_obsidian:` 与路径。

- [ ] **Step 5: `cargo test -p talksage-knowledge --lib` 全绿。**

---

### Task 2: 配置迁移到 `plugins.knowledge_obsidian`

**Files:**
- Modify: `crates/talksage-config/src/lib.rs`
- Modify: `crates/talksage-config` 内现有 merge/load 测试

- [ ] **Step 1: 写失败测试** — 仅含

```toml
[knowledge_base]
enabled = true
folder = "D:\\Obsidian"
```

的用户文件，合并后 `plugins.get_string("knowledge_obsidian", "folder")`（或等价 API）为该路径且 enabled 为 true。再测：若 `plugins.knowledge_obsidian.folder` 已非空，则**不**用旧键覆盖。

- [ ] **Step 2: 跑测试，确认失败。**

- [ ] **Step 3: 在 merge 用户配置处实现迁移。** `apply_updates` 的 `knowledge_base` 分支继续可用，同时写入 `plugins.knowledge_obsidian`。`AppConfig.knowledge_base` 可作为只读投影（`enabled`/`folder` 映射源插件），避免设置页一次改不完就坏。投影与 plugins 表不得互相打赢：以 plugins 为准。

- [ ] **Step 4: 配置 crate 相关测试全绿。**

---

### Task 3: 源插件注册 + `brief_retriever` 默认关

**Files:**
- Create: `crates/talksage-plugins/src/knowledge_obsidian.rs`
- Modify: `crates/talksage-plugins/src/lib.rs`、`registry.rs`、`builtin.rs`、`brief_retriever.rs`
- Modify: `crates/talksage-plugins/src/builtin.rs` 测试（id 唯一、`enabled` 默认、analysis 列表不含源插件）

- [ ] **Step 1: 写失败测试** — `builtin_plugins()` 含 `knowledge_obsidian`；其 `category == KnowledgeSource`；`analysis_plugin_ids()` **不含**它。`BriefRetrieverPluginDef.default_config()` 的 `enabled == false`。`register` 后 observer 数量不因源插件增加。

- [ ] **Step 2: 跑测试，确认失败。**

- [ ] **Step 3: 增加** `PluginCategory::KnowledgeSource`、`PluginPhase::Source`（`as_str` 必须穷尽）。源插件 `default_config`: `{ enabled, folder }`；`capabilities: &[]`；`register` 空实现。插入 `builtin_plugins()` 末尾（不参与 filter 顺序不变量则不要插到 short_segment 前面）。`plugin_metadata` 已有 `category` 字段即可，确认 `analysis: false`。

- [ ] **Step 4: `cargo test -p talksage-plugins` 全绿。** 前端 `pluginStatusLabel` 若写死 capability 文案，缺路径时用设置页自定义说明，不必强行给源插件加 `KnowledgeBase` capability（源是供给方）。

---

### Task 4: `KnowledgeHub` 与 pipeline 共用索引

**Files:**
- Create: `crates/talksage-pipeline/src/knowledge.rs`
- Modify: `crates/talksage-pipeline/src/lib.rs`、`service.rs`
- Modify: `web/src-tauri/src/lib.rs`、`crates/talksage-server/src/lib.rs`（构造 Hub 并注入）
- Test: `crates/talksage-pipeline` 单测或 `knowledge.rs` 内测

- [ ] **Step 1: 写失败测试** — 临时 vault + `Config` 启用源插件；`KnowledgeHub::refresh` 后 `is_ready()`；`search` 能命中笔记用语。禁用 enabled 后 refresh，`is_ready()==false`。`kb_folder_override` 仍覆盖根路径（CLI/测试）。

- [ ] **Step 2: 跑测试，确认失败。**

- [ ] **Step 3: 实现 Hub：** 读 `plugins.knowledge_obsidian`（及迁移后的投影）；`Mutex<Arc<KnowledgeIndex>>` + 指纹（enabled+folder）避免每段重读。`TalkSageService` 持有 `Arc<KnowledgeHub>`；`plugin_registrations` 与 `start_listen` 用 `hub.is_ready()` / `hub.index()` 填 `PluginContext.kb`，**删除** service 里直接 `KnowledgeBase::index_folder`。开始监听时 `refresh_if_stale`。

- [ ] **Step 4: 装配 Tauri `AppState` 与 ServerState：Hub 单例，与 `ChatService` 共享 Arc。** 现有 pipeline/server 测试若构造 Service，补上 Hub（可用默认空索引）。

- [ ] **Step 5: `cargo test -p talksage-pipeline` 以及会碰到构造函数的 `talksage-server` 测试要绿。**

---

### Task 5: 纪要 / 智能纪要注入 `{knowledge}`

**Files:**
- Modify: `crates/talksage-notes/prompts/notes_system.txt`、`notes_user.txt`、`trio_*_system.txt`、`trio_user.txt`
- Modify: `crates/talksage-notes/src/lib.rs`
- Modify: 调用方 `web/src-tauri/src/lib.rs`、`crates/talksage-server/src/lib.rs`、`crates/talksage-cli/src/session_cli.rs`
- Test: `crates/talksage-notes/src/lib.rs` 现有 `prompt_templates_keep_required_placeholders` 与 mock LLM 测试

- [ ] **Step 1: 写失败测试** — placeholders 含 `{knowledge}`。Mock LLM：`generate(..., knowledge)` 的 user prompt 含知识块；`knowledge` 为空字符串时仍成功且 prompt 含「（无相关知识）」或等价。Trio：三次 `complete` 的 user 参数含**同一** knowledge 字符串（mock 记录三次 user prompt）。

- [ ] **Step 2: 跑测试，确认失败。**

- [ ] **Step 3: 实现签名** `knowledge: &str`。System 增加：只能用知识块中的事实，没有则写未记载，不要编造政策。User 模板增加 `## 相关知识` `{knowledge}`。

- [ ] **Step 4: 调用方** 用本场 title + meeting_description + 要点文本拼 query（过短则追加转写前约 1500 字），`hub.search(..., 8, min_score)` → `format_knowledge_block`。Hub 未 ready 则空块，**生成不失败**。抽出 `fn notes_knowledge_query(...)` 便于单测截断。

- [ ] **Step 5: `cargo test -p talksage-notes` 全绿；CLI/server 能编译。**

---

### Task 6: AI 助手按轮注入

**Files:**
- Modify: `crates/talksage-pipeline/src/chat.rs`
- Test: 同文件；抽出纯函数测 prompt，不必真打 LLM

- [ ] **Step 1: 写失败测试** — `system_prompt_for_turn(base, knowledge_block)`：block 空则等于 `SYSTEM_PROMPT`；非空则包含路径与「区分引用与推断」。`send` 无 LLM 的现有测试仍通过。

- [ ] **Step 2: 跑测试，确认失败。**

- [ ] **Step 3: `ChatService` 增加 `knowledge: Arc<KnowledgeHub>`（或 `Option`，测试用空 Hub）。** `send` 在组 messages 前 `refresh_if_stale`，用本轮用户原文 `search(..., 5, min_score)`，把 block 拼进**同一条** system。无命中不改 system。

- [ ] **Step 4: 更新 Tauri/Server 构造。** `cargo test -p talksage-pipeline chat` 相关测试全绿。

---

### Task 7: 会中材料包 + 右栏 + 设置文案

**Files:**
- Modify: `crates/talksage-pipeline/src/service.rs`（`StartListen.pinned_note_paths`）
- Modify: `crates/talksage-session/src/lib.rs`（`SessionMeta.pinned_note_paths: Vec<String>`，`#[serde(default)]`）
- Modify: `web/src-tauri/src/lib.rs`（`list_knowledge_documents`、`start_listen` 参数）
- Modify: `crates/talksage-server/src/lib.rs`（对等 HTTP，若 listen/start 有 JSON body 则加上；否则至少 Tauri）
- Modify: `web/src/App.tsx`、`web/src/components/AsidePanel.tsx`、`web/src/lib/api.ts`
- Modify: `web/src/sections/SettingsSection.tsx`、`web/src/lib/knowledge.ts`、对应 `*.test.ts`
- Modify: `crates/talksage-pipeline/src/finalize.rs` 若在此写 meta：把钉住路径合并进 meta，勿覆盖质量字段

- [ ] **Step 1: 后端测试** — `list_documents` 去重；`StartListen` 带不存在的 path 时不 panic；meta 反序列化旧 JSON（无该字段）成功。

- [ ] **Step 2: 实现列表 API 与监听参数。** 右栏默认渲染钉住笔记正文（从 index 按 path 取 chunk 拼接，截断与现 Brief 卡片类似）。空态文案按 spec。自动 `Brief` 仍可显示，但默认插件关闭后不会出现。

- [ ] **Step 3: 设置页** 改为「知识源：Obsidian」；绑定 `plugins.knowledge_obsidian`（或投影 `knowledge_base` 但保存写 plugins）。说明改为：保存后刷新索引，供材料包、纪要、助手使用。去掉「下次监听才生效」若 Hub 已在保存时 refresh——保存配置后调用一次 `hub.refresh()`。

- [ ] **Step 4: 转写页** 开始监听前可多选文档（简单列表即可，不做完整文件树）。`startListen(pinned_note_paths)`。新监听清空自动 briefs，材料包用本次选择。

- [ ] **Step 5: 前端单测**（`knowledge.ts` / plugins 文案）更新。`npm test` 或仓库现有 web test 命令。无法开桌面应用时，至少 API 层与纯函数测过，并在回复里写明未做浏览器手点。

---

### Task 8: 示例配置与文档

**Files:**
- Modify: `config/talksage.example.toml`
- Modify: `docs/architecture-v2.md` §8.7
- Modify: `docs/plugin-development.md`（KnowledgeSource、禁止 observer 里建索引）
- Modify: `README.md`、`README_zh-CN.md` 知识库一句

- [ ] **Step 1:** example 增加 `[plugins.knowledge_obsidian]`；保留 `[knowledge_base]` 注释「已迁移，仍可读」。`[brief_retriever] enabled = false`。

- [ ] **Step 2:** 文档与 spec 一致：三层、一期一个 vault、会中材料包、会后 RAG。

- [ ] **Step 3:** `cargo test -p talksage-knowledge -p talksage-plugins -p talksage-config -p talksage-notes -p talksage-pipeline`（以及会受影响的 server 测试）全绿。

---

## 验证清单

- 旧 toml 只有 `[knowledge_base]` 时能索引。
- 纪要无命中仍能生成；有命中 prompt 含相对路径。
- 助手无命中 messages 与现在相同（system 原文）。
- 会中不钉笔记时右栏空态，不会刷无关 Brief（默认关自动卡）。
- 场景 allowlist 不能关掉 Obsidian 源。
- 未实现：第二 vault、历史会话源、向量、highlights RAG。
