# 知识库：源插件 + 检索底盘 + 三类消费者

**日期：** 2026-08-29
**状态：** 待实施
**对照：** [architecture-v2.md](../../architecture-v2.md) §8.7（现实现为 Jaccard/词法检索 + `brief_retriever` 贴原文，本文将替换其产品语义）；[plugin-development.md](../../plugin-development.md)

本文是产品与架构契约。实现计划：`docs/superpowers/plans/2026-08-29-knowledge-base.md`。

---

## 1. 问题

现有「知识库」把三件不同的事焊在一起：

1. **从哪读材料**（本机文件夹 / Obsidian）写死在 `TalkSageService` 启动监听时 `index_folder`；
2. **怎么检索**关在 `talksage-knowledge`，只被会中 observer 调用；
3. **给谁用**只有实时转写右栏「知识库命中」（`brief_retriever` 把 chunk 原文堆上去）。

后果：

- 纪要 / 智能纪要 / AI 助手完全不读库，生成只能靠本场转写；
- 会中每段 ASR final 都当查询，假阳性伤接话；
- 换一种文件源或加第二个来源，只能改 pipeline，没有扩展点；
- `PluginCapability::KnowledgeBase` 表示「监听时有没有索引」，不是「产品有没有知识库」。

产品定位仍是：听对方讲话 → 关联我的知识 → 支撑当场回复；会后生成也要能引用同一份知识。RAG（检索后写入 LLM）是会后手法，不是会中默认管线。

---

## 2. 目标与非目标

**目标（一期）**

- 知识**源**可插拔：编译期注册、运行时配置，形态与现有内部插件一致（不是动态 dll）。
- 一期只实现 **本地 Obsidian，一个 vault 路径**。
- 统一 **检索底盘** `retrieve(query)`，返回带 `source_id` 与仓库相对路径的片段。
- 三个消费者都接上这条底盘：
  - **实时转写：会中接话** — 材料包（从该 vault 钉笔记）优先；自动检索卡片默认关。
  - **纪要 / 智能纪要** — 生成前 retrieve **一次**，注入 prompt，要求引用、禁止编造。
  - **AI 助手** — 每轮按用户问题 retrieve，有命中才注入；助手仍不绑定某场 session。
- 配置从 `[knowledge_base]` 迁到源插件；旧 toml 能读进来。

**非目标（一期明确不做）**

- 第二个 vault、同一源类型多实例。
- 第二种源插件（普通文件夹、历史会话、云笔记）。接口必须能加，实现后做。
- 向量库、embedding、会中每段 RAG（对命中片段再 LLM 压缩）。
- Obsidian 官方 API、双链解析、块引用、`.canvas` / 附件。
- 动态加载第三方源。
- 用会中 `DomainEvent::Brief` 作为纪要/助手的依据（不入库，假阳性会污染会后）。
- 改「立即整理」`generate_highlights`（避免一期范围膨胀）。
- 让 AI 助手自动挂上当前历史会话转写（那是未来「会话源」或显式上下文，不是 Obsidian 源的职责）。

---

## 3. 架构

三层，禁止再把索引写进会中 observer。

```text
源插件（Obsidian …）
    → 片段 { source_id, path, heading, text }
检索底盘 KnowledgeIndex
    → retrieve(query) → hits { source_id, path, heading, text, score }
消费者
    → 会中：展示材料包 / 可选自动卡
    → 纪要：{knowledge} 注入，一次检索，三路智能纪要共享
    → 助手：有命中才注入 context
```

### 3.1 源插件 ≠ SegmentObserver

现有 `Filter` / `Observer` / `Finalizer` 吃的是转写生命周期。知识源吃的是「配置变更 / 首次使用 / 开始监听 / 会后生成前」的刷新。

源插件：

- 使用现有 `Plugin` 身份与配置元数据（`id`、`default_config`、设置页 schema），`category = KnowledgeSource`（新增变体）。
- **不**加入场景 `plugin_allowlist`（那只约束分析类会中功能）。
- `register(hooks)` **不**往 `HookRegistry` 挂 filter/observer/finalizer。
- 宿主根据该插件的 `PluginConfig` 构造 `dyn KnowledgeSource`，把片段交给底盘。

一期内置一条：`knowledge_obsidian`（label：Obsidian 仓库）。

### 3.2 源契约（`talksage-knowledge`）

```rust
pub struct KnowledgeSnippet {
    pub source_id: String, // 稳定 id，一期固定 "knowledge_obsidian"
    pub path: String,      // vault 相对路径，展示与引用用
    pub heading: String,   // 所在标题；没有则为空
    pub text: String,
}

pub trait KnowledgeSource: Send + Sync {
    fn id(&self) -> &'static str;
    /// 读盘并切块。失败要可诊断（路径不存在、不是目录）。
    fn load_snippets(&self) -> anyhow::Result<Vec<KnowledgeSnippet>>;
}
```

`ObsidianSource { root: PathBuf }`：

- `root` 必须是已存在的目录，否则 `load_snippets` 返回错误（设置页/doctor 可显示），索引视为空。
- 递归索引 `.md` 与 `.txt`（与现有 walker 一致）。Obsidian 以 markdown 为主；同目录纯文本一并纳入，避免用户已有 `.txt` 笔记突然消失。
- 目录名跳过：`.obsidian`、`.trash`、`.git`（与现实现一致）。
- 切块规则沿用现有 `chunk_file`（按 `#` / `##` / `###`，过长再按段）。

### 3.3 检索底盘

现有 `KnowledgeBase` **演进为索引**（可改名 `KnowledgeIndex`，或保留类型名但语义改为「多源片段索引」）：

- `rebuild(snippets)`：清空后写入，重建 token / IDF（沿用现有中文 2-gram、词组门槛、口水词 DF 过滤）。
- `search(query, top_k, min_score) -> Vec<KnowledgeHit>`：`KnowledgeHit` 必须带 `source_id` 与 `path`（今日 `source` 字段即文件相对路径，补上 `source_id`）。
- `is_ready()`：`chunk_count() > 0`。
- `list_documents() -> Vec<{ path, title }>`：供材料包挑选；`title` 取文件第一标题或文件名。同一 path 多 chunk 去重为一条文档。

检索策略一期不换：词法 + 词组门槛。向量是来源变多、同义不够时再加，不在本文范围。

**索引所有权：** `TalkSageService`（或与之同级的单一持有者）持有 `Arc<KnowledgeIndex>`。监听中的 `PluginContext.kb`、纪要生成、助手发送，都读这一份，禁止各消费者各索引各的。

**何时刷新：**

| 时机 | 行为 |
|---|---|
| 源插件 enabled 且 folder 有效，配置保存成功 | 后台或同步重建；失败记日志，`is_ready()==false` |
| 开始监听 | 若索引空或配置变更过，再刷新一次 |
| 生成纪要 / 智能纪要 / 助手提问 | 若未 ready 且源已启用，先刷新再 retrieve；仍空则按「无知识」继续（纪要/助手不报错） |
| 源 disabled 或路径清空 | 清空索引 |

不在每一段 ASR、每一轮聊天都重读整个 vault。

### 3.4 宿主装配

`CapabilityAvailability.knowledge_base == index.is_ready()`。会中自动卡 observer 仍可依赖该 capability；纪要/助手不走插件注册表，直接问服务要 index。

CLI 现有 `kb_folder_override`：一期仍表示「覆盖 Obsidian 根路径」，仅用于测试/命令行，不引入第二源。

---

## 4. 配置与迁移

**新配置**（与其它插件一样落在通用 `plugins` 表）：

```toml
[plugins.knowledge_obsidian]
enabled = false
folder  = ""    # 一个 vault 根路径
```

设置页：开关 + 路径 + 浏览文件夹（现有 `pick_folder`）。文案改为「知识源：Obsidian」，不要写「启用简报检索」。保存后应刷新索引（不必等到下次监听）。

**旧配置：**

```toml
[knowledge_base]
enabled = false
folder  = ""
```

加载合并规则：若 `plugins.knowledge_obsidian.folder` 为空且 `[knowledge_base].folder` 非空，则把 `enabled`/`folder` 拷到源插件。保存更新时写新键；读取仍接受旧键，避免本机已有 `talksage.toml` 失效。

`AppConfig.knowledge_base` 可保留为兼容投影（读写都转到源插件），或加载后只存在于 plugins 表。测试必须锁住：只含旧 `[knowledge_base]` 的文件启动后，源插件能索引。

`[brief_retriever] enabled` 默认改为 **false**（自动命中卡片默认关）。已有用户 toml 里若显式 `enabled = true` 则尊重。材料包不依赖该插件。

场景 allowlist 里的 `brief_retriever` 一期可保留（自动卡仍受场景约束）；材料包不受场景 allowlist 关闭（源启用且用户钉了笔记就显示）。

---

## 5. 消费者行为

### 5.1 会中接话（实时转写）

**材料包（一期要做）**

- 转写页在开始监听前（或监听中）从当前 vault 的文档列表里多选钉住；路径为 vault 相对路径。
- 开始监听把钉住列表带进会话（`ListenRequest` 增加 `pinned_note_paths: Vec<String>`；非法/已删除路径静默跳过）。
- 右栏「知识库」**默认展示钉住笔记的正文**（按篇，标题 + 截断，可滚动）。不是搜索命中列表。
- 钉住列表写入 `sessions.meta`（或等价字段），便于历史回看这场会带着哪些笔记。没有钉住时右栏空态：「从知识库钉住笔记，会中可对照」+ 入口。

**自动检索卡片**

- 现 `brief_retriever` 降为可选：默认关；打开后行为可保持「高阈值 search top 2」，但不得替代材料包，也不得再作为唯一 UI。
- `DomainEvent::Brief` 仍不写 SQLite（Replayable）。会后 RAG 不得读取这些事件。

会中 **默认不调用 LLM** 压缩命中。

### 5.2 纪要 / 智能纪要

入口不变：历史页、Tauri、HTTP、CLI。生成前：

1. 用本场 **标题 + 用户填的会议说明（若有）+ 要点文本** 拼查询；若仍过短，再追加转写前 N 字（N 实现时取约 1500 字，测试锁住截断存在即可）。
2. `retrieve` 一次，`top_k` 建议 8。
3. 格式化知识块（无命中则为「（无相关知识）」或省略该节且 system 说明「无知识块时不要捏造公司政策」）。
4. `NotesGenerator::generate` 与 `TrioGenerator::generate` 增加 `knowledge: &str`（或 `Option<&str>`）。**智能纪要三路并行共享同一字符串**，禁止三路各 search 一次。

知识块格式（示例）：

```text
## 相关知识（来自用户知识库；只能引用下列片段中的事实，没有的写「转写中未提及/知识库未记载」）
### knowledge_obsidian:200-Wiki/客户A.md — 商务条款
…
```

System prompt 增加同等约束。现有 `{transcript}` `{key_points}` 等占位符保留；新增 `{knowledge}`。`prompt_templates_keep_required_placeholders` 测试必须包含 `{knowledge}`。

无索引或无命中：**仍生成纪要**，不失败。

### 5.3 AI 助手

`ChatService::send` 在组 messages 时：

- `query = 本轮用户原文`（不把最近 20 条整段拿去搜，避免串题）。
- `retrieve`，`top_k` 建议 5。
- **无命中：消息列表与现在完全相同**（固定人设 + 话题历史）。
- **有命中：** 把知识块与引用约束 **追加到同一条 system prompt**（不新增第二条 system、不伪装成 user 消息，避免兼容端点只认一条 system 或把 user 上下文当成提问）。然后才是话题历史与本轮提问。
- 流式 `ChatDelta`、取消、落库路径不变。
- 助手话题仍不绑定 `session_id`。

---

## 6. UI 与 API

| 能力 | 说明 |
|---|---|
| 设置 → 知识源 Obsidian | enabled、folder、浏览文件夹；保存触发刷新 |
| 转写页材料包 | 列出 vault 文档、多选钉住、右栏展示 |
| `list_knowledge_documents` | Tauri / HTTP：文档 `{ path, title }[]`；源未就绪返回 `[]` |
| 开始监听 | 请求体或 IPC 带上 `pinned_note_paths` |
| 插件状态 | `knowledge_obsidian` 显示为知识源；缺路径/空库 → unavailable 或等价文案（「未索引到笔记」） |

设置页去掉「会中由简报检索插件检索此目录」这种把源和 observer 绑死的说明。

---

## 7. 测试契约（必须有自动化）

- Obsidian 源跳过 `.obsidian` / `.trash` / `.git`；只从 vault 相对路径引用。
- 空目录 / 不存在路径：`is_ready()==false`，不 panic。
- 旧 `[knowledge_base]` 合并进 `knowledge_obsidian`。
- `retrieve` 命中含 `source_id`。
- 纪要 prompt 含知识块；无命中仍成功；trio 三路看到的 knowledge 字符串相同（可用 mock LLM 断言三次 complete 的 user prompt 含同一块）。
- 助手：无命中时发给 LLM 的 messages 不含知识块；有命中时含路径。
- 材料包：`list_documents` 对同一文件去重；钉住缺失路径不炸。
- `brief_retriever` 默认配置 `enabled == false`。

---

## 8. 文档与示例

实施时同步：

- `config/talksage.example.toml`：源插件 + 迁移注释；`brief_retriever` 默认 false。
- `docs/architecture-v2.md` §8.7：改为源插件 + 底盘 + 三消费者；删除「仅 Jaccard 简报」作为终态描述。
- `docs/plugin-development.md`：增加 KnowledgeSource 类别与「不要在 observer 里 index_folder」。
- README 功能列表：知识库改为「Obsidian 源；会中材料包；纪要/助手可引用」，不要只写「知识库简报命中」。

---

## 9. 落地顺序

1. 源契约 + Obsidian 单 vault + 底盘刷新/retrieve + 配置迁移。
2. 纪要 / 智能纪要注入。
3. 助手按轮注入。
4. 会中材料包 + 自动卡默认关 + 设置文案。

每步都应可单独测试、可运行。不要先做向量或第二种源。

---

## 10. 开放问题（一期已关闭）

| 问题 | 结论 |
|---|---|
| 实时要什么 | 会中接话，材料包优先 |
| 非实时谁用 | 纪要/智能纪要 **和** AI 助手 |
| 多源 | 插件接口；一期只有 Obsidian 一个路径 |
| 历史会话当源 | 不做；未来新源插件 |
| 会中 RAG | 默认不做 |
