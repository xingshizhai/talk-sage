# TalkSage 插件化架构设计（everything is a plugin）

**日期：** 2026-08-20
**状态：** 已评审，待实施
**对照：** [architecture-v2.md](../../architecture-v2.md)（已实施的 v2 骨架）
**参考：** deepseek-harness / Cordis 的 capability seam 模型

---

## 1. 问题

当前只有一个扩展点 `AnalyzerPlugin`：看一条 committed 段 → 吐一个 `DomainEvent`。其余「插件形状」的功能都硬编码在核心里：

| 功能 | 当前位置 | 问题 |
|---|---|---|
| `min_segment_ms` 短段抑制 | `pipeline/src/lib.rs:646`（`StreamWorker` 内） | 只能靠集成测试间接覆盖 |
| 跨流回声去重 | `pipeline/src/lib.rs:812` | 同上 |
| 会话指标 / 教练提示 | `pipeline/src/lib.rs` 调度 | 与采集/VAD/ASR 挤在同一个 1130 行文件 |
| 会话质量评估 | `session/src/lib.rs:428` + `service.rs` | 判定与调度分离，难改 |
| webhook | `service.rs` | — |
| Markdown 导出 | `server/src/lib.rs` 与 `web/src-tauri/src/lib.rs` 的 handler | ~~两份实现~~ **勘误：并非重复。** 实现只有一份（`talksage_session::export_markdown`），两个 handler 是薄封装，差异为真实产品差异（桌面端额外落文件）。无需处理 |

后果有三：

1. **加一个插件要改三处** —— pipeline 组装、`PluginsConfig` 结构体、设置 UI。
2. **`pipeline/src/lib.rs` 1130 行**，采集、VAD、ASR、录音、声纹、指标、插件调度全在里面。
3. **快慢路径没有类型隔离** —— 慢操作混进实时路径只能靠约定和 code review 拦。

## 2. 目标与非目标

**目标**

- 把上述 8 个硬编码功能搬到统一的插件接缝，另有 3 个现有插件（术语/翻译/简报）改名接入，合计 11 个插件
- 加插件的成本降到「加 1 个文件 + 注册表里 1 行」
- 用类型系统表达快/慢路径的性能契约

**非目标（本轮明确不做）**

- **第三方插件 / 动态加载**。Rust 无稳定 ABI，dylib 跨版本必崩；WASM 宿主是独立产品项。插件全部在仓库内，编译期注册。
- **完整 Cordis 化**：不做服务注册表、不做 `inject` 声明式依赖、不做四种事件派发模式。
- **新增 crate**。接缝以现有 crate 的 module 出现。
- **把智能标点搬到 Rust**。它在 `web/src/lib/transcript.ts:148`，是前端展示逻辑；architecture-v2 §6 明确要求「智能标点不得直接改已持久化文本」，搬成 Rust EventFilter 会改变落库内容。标点留在前端。

## 3. 架构

### 3.1 核心抽象

新增 `talksage-plugins/src/registry.rs`：

```rust
/// 插件：拥有身份与默认配置，在 register() 里把自己挂进需要的钩子。
/// 对应 Cordis 的「插件注册进 seam，而不是拥有 seam」。
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn default_config(&self) -> PluginConfig;
    fn register(&self, cfg: &PluginConfig, hooks: &mut HookRegistry);
}

/// 快路径：每个事件都过。签名里没有 PluginContext，也没有 Result ——
/// 类型上堵死在热路径做慢活或失败重试。
pub trait EventFilter: Send + Sync {
    /// 返回 None = 吞掉该事件。
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent>;
}

/// 慢路径：committed 段触发。skeleton 同步无 HTTP，run 在独立线程可含 LLM。
/// 即今日的 AnalyzerPlugin。
pub trait SegmentObserver: Send + Sync {
    fn should_trigger(&self, seg: &TranscriptSegment) -> bool;
    fn accepts_speculative(&self) -> bool { false }
    /// 阶段 3 起返回 Vec：一段上可能同时产出多个事件（指标 + 提示），
    /// Option 表达不了。空向量 = 不发。
    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent>;
    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent>;
}

/// 会后：stop → flush → 写入 barrier 之后跑，不占实时路径。
pub trait SessionFinalizer: Send + Sync {
    fn finalize(&self, ctx: &FinalizeContext) -> anyhow::Result<()>;
}

pub struct HookRegistry {
    filters: Vec<Arc<dyn EventFilter>>,
    observers: Vec<Arc<dyn SegmentObserver>>,
    finalizers: Vec<Arc<dyn SessionFinalizer>>,
}

/// finalizer 的输入：会话已 flush、已落库，此处只读。
pub struct FinalizeContext<'a> {
    pub session_id: Option<i64>,
    pub transcript: &'a TranscriptState,   // committed 段
    pub stats: &'a [SessionStats],         // 每条流一份
    pub quality: SessionQuality,           // 由 session_quality 插件先行写入
    pub config: &'a ConfigManager,
    pub llm: Option<Arc<dyn LLMProvider>>,
    pub data_dir: &'a Path,
}
```

`PluginContext`（`kb` + `llm`）沿用现状不变，仅供 `SegmentObserver::run` 使用。

一个插件可以挂多个钩子。`session_quality` 同时是 Observer（跨段累计）和 Finalizer（会后判定），因此它必须排在 finalizer 链首位 —— 其余 finalizer 依赖它写入的 `quality` 字段。

### 3.2 注册机制

显式中心表，不用 `inventory` / `linkme`：

```rust
/// 内置插件清单。顺序即 EventFilter 链的执行顺序。
pub fn builtin_plugins() -> Vec<Box<dyn Plugin>> { /* ... */ }
```

插件全在仓库内且都会被链接，链接器分布式注册带来的「无中心列表」收益，不抵它在交叉编译和 `--gc-sections` 下的坑。加插件 = 加一个文件 + 表里一行。

### 3.3 数据流

```
StreamWorker 产生 final 段
  → EventFilter 链（按 builtin_plugins() 的列表顺序）
       short_segment      : final 段 duration < min_ms → None
       cross_stream_dedup : 与另一条流重复 → None
  │  被吞掉的事件到此为止：既不 emit，也不触发 observer
  ↓
  → TranscriptState.apply + revision 戳（现有 runtime.rs）
  → SessionWriter 落库（只 committed）
  → SegmentObserver 分发
  → EventSink → Tauri IPC / WebSocket

会话 stop → flush → 写入 barrier →
  SessionFinalizer 链：session_quality → webhook
  （导出与纪要是按需 API，不在此链上 —— 见 §3.5 勘误）
```

**filter 挂在事件产生点，不是 sink。** 必须显式说明，因为现状两个待迁移功能的作用域不一致：

| 功能 | 现状抑制事件 | 现状抑制插件 |
|---|---|---|
| `short_segment`（`lib.rs:646` 提前 `return`） | 是 | 是 |
| `cross_stream_dedup`（emit 包装内 `return`） | 是 | **否** |

`on_final`（插件分发）在 `lib.rs:713` 紧跟 `emit` 之后调用，绕过了 emit 包装。因此跨流回声重复段虽不进 UI 和数据库，却仍会触发术语/翻译/简报插件，可能带来重复的 LLM 调用 —— 这是既存的隐性缺陷。

若把 filter 挂在 sink 上，`short_segment` 会退化成与 dedup 一致（短段开始触发插件），属于回归。因此 filter 统一放在产生点，位于 `emit` 与 `on_final` 之前。

**已决策的行为变更：** 本次重构顺带修正上述缺陷 —— 跨流回声重复段从此不再触发插件。特征化测试（§6）须将这一差异记为预期变更，而非回归。

### 3.4 两条定死的语义

**S1. filter 只作用于采集/ASR 产生的事件。** 插件自己 emit 的事件（`Metrics`、`Nudge`、`Term`、`Translation`）直接进 sink，不回灌 filter 链。否则形成递归且难以推理。质量门控发生在 finalizer 之前的 `skip_analysis`，不依赖 filter。

**S2. 钩子顺序 = `builtin_plugins()` 的列表顺序。** 不引入优先级数字或声明式依赖。顺序敏感的有两处，均由列表位置保证：

- filter 链：`short_segment` 必须在 `cross_stream_dedup` 之前（便宜的先跑，且 dedup 需要看两条流的历史）
- finalizer 链：`session_quality` 必须在 `webhook` 之前（前者把质量 meta 写进会话行，后者重新读这一行来拼载荷；耦合走数据库，不走 `FinalizeContext` —— 那是个只读引用，finalizer 之间不经由它传值）

写在一个列表里比分散声明更容易审查；§6 的注册表不变量测试会锁住这两条相对顺序。

### 3.5 插件清单

| id | 钩子 | 迁移来源 |
|---|---|---|
| `short_segment` | Filter | `pipeline/src/lib.rs:646` |
| `cross_stream_dedup` | Filter | `pipeline/src/lib.rs:812` |
| `term_explainer` | Observer | 现有，仅改名 |
| `translator` | Observer | 现有，仅改名 |
| `brief_retriever` | Observer | 现有，仅改名 |
| `conversation_metrics` | Observer | `pipeline/src/lib.rs` 调度。**含教练提示** —— 原设计列为独立的 `coaching_nudge`，实施时合并：两者共享 `seg_log`，拆开会让本已是 O(n²) 的指标计算翻倍 |
| `session_quality` | Observer + Finalizer | `session/src/lib.rs:428` + `service.rs` |
| `webhook` | Finalizer | `service.rs` |
| ~~`markdown_export`~~ | ~~Finalizer~~ | **勘误：不是 finalizer。** 它是用户在历史页点击触发的按需 API（`GET /session/{id}/export` 与对应 Tauri 命令），`finish()` 从不调用。真实问题是 server 与 tauri 各有一份实现 —— 解法是抽成 `TalkSageService` 共享方法，与插件无关 |
| ~~`trio_notes`~~ | ~~Finalizer~~ | **勘误：同上。** 按需触发且含 LLM 调用；做成 finalizer 等于每次会话结束都自动烧 token，与「用户点击才生成」的产品行为相抵触 |

## 4. 配置与 UI

```toml
[plugins.term_explainer]
enabled = true
cooldown_seconds = 30

[plugins.short_segment]
enabled = true
min_ms = 300          # 由 [audio] min_segment_ms 迁入插件名下
```

`PluginConfig` 用 `serde_json::Value` 包一层 + 类型化取值器（`get_bool` / `get_f64` / `get_str`），与 `ConfigManager` 已有的 `apply_scene_params(p, u: &serde_json::Value)` 模式一致，不引入新的 schema 机制。

**合并顺序：** `plugin.default_config()` → `[plugins.<id>]` 用户值覆盖 → 场景模式的 **allowlist** 最后裁决 `enabled`。

统一用 allowlist（不用 denylist）：每个场景显式列出允许启用的插件 id，不在列表里的一律关闭。新增插件默认不会因为某个场景忘了更新而意外开启。生活模式关闭分析插件的行为不变，只是从「if 链里写死」变成「场景提供一份 id 列表」。

**破坏性变更：** `talksage-config` 中 `PluginsConfig` 的具名字段结构（`term_explainer` / `translator` / `brief_retriever` 三个 `PluginToggle`）被通用表取代，且不做读时迁移。现有用户 `talksage.toml` 里的这三个开关与 cooldown 值回落到插件默认值。会议场景默认全开，实际影响小。此项为已决策的取舍。

**设置 UI：** 新增 `GET /plugins`（server）与对应 Tauri command，返回 `[{id, enabled, schema: default_config}]`；`SettingsSection.tsx` 改为按元数据生成表单。加插件不再需要改前端。

## 5. 错误处理

| 钩子 | 契约 |
|---|---|
| `EventFilter::filter` | 无 `Result`、无 `PluginContext`。纯函数、不可失败、不可阻塞 |
| `SegmentObserver::skeleton` | 同步、本地、无 HTTP |
| `SegmentObserver::run` | 独立线程；`ureq` 15s 超时；会话已停则丢弃迟到结果（补显式 cancel token） |
| `SessionFinalizer::finalize` | 返回 `Result`，逐个独立执行。webhook 失败不得阻塞导出；失败记日志并继续，最后汇总报告 |

finalizer 全部在 `stop → flush → 写入 barrier` 之后执行。这是 architecture-v2 §8.2 已有的要求，本设计将其由约定改为类型位置强制。

## 6. 测试策略

主要风险是**行为漂移**：搬动 8 个功能，任一语义改变都是回归。三层防护：

1. **特征化测试先行（阶段 1 完成前写）。** 用 `pipeline_live` 的固定语料跑一遍，把完整事件序列快照下来；重构后逐事件比对，不允许差异。这是证明「只是搬家、没改语义」的唯一手段。
2. **注册表不变量。** id 唯一；`default_config()` 可解析；filter 链与 finalizer 链的实际顺序与 `builtin_plugins()` 声明一致，且锁住 §3.4 的两条相对顺序。
3. **插件单测。** `short_segment` 与 `cross_stream_dedup` 从 `StreamWorker` 剥离后成为纯函数，首次可脱离真实 ASR 单独测试。

现有集成测试 `min_commit_ms_suppresses_short_segments` 与 `cross_stream_echo_dedup_keeps_single_copy` 保持不动，作为迁移的回归网。全程要求 `TALKSAGE_REQUIRE_MODELS=1` 下测试全绿、零跳过。

## 7. 分阶段迁移

每阶段独立可提交、测试全绿。

| 阶段 | 内容 | 风险 | 状态 |
|---|---|---|---|
| 1 | 特征化测试 + `registry.rs` 骨架 + `AnalyzerPlugin` → `SegmentObserver` 接入 | 低，纯搭台 | ✅ 完成 |
| 2 | filter 链：`short_segment` / `cross_stream_dedup` 搬出 `StreamWorker` | 中，动热路径 | ✅ 完成 |
| 3 | observer：`conversation_metrics` 搬出（含 nudge） | 中，跨段状态 | ✅ 完成 |
| 4 | finalizer：`session_quality` / `webhook` / `markdown_export` / `trio_notes`；server 与 tauri 导出合一 | 中，涉及两个适配器 | 待办 |
| 5 | 配置换通用表 + `/plugins` 元数据端点 + 设置 UI 自动生成 | 低，但面广 | 待办 |

**预期产出：** `pipeline/src/lib.rs` 由 1130 行降至约 800 行；server 与 tauri 各去掉一份导出实现；`plugins/` 下新增 6 个小文件。

### 阶段 1–2 实施记录（2026-08-20，分支 `refactor/plugin-registry`）

| 指标 | 预期 | 实际 |
|---|---|---|
| `pipeline/src/lib.rs` 行数 | 阶段 1–2 后约 1070 | **1165**（1130 → 1165，净增 35 行） |
| 加 filter 插件的成本 | 加 1 文件 + 表里 1 行 | ✅ 达成（pipeline 内无任何具体插件类型引用，仅一处解释性注释） |
| 特征化 golden | 保持不变 | ✅ 字节一致，全程未重新生成 |

> **勘误。** 本节初版记为「1130 → 1119，仅减 11 行」，那是在三个复查修复
> （`c63f0a0` / `03bc360` / `5604e95`）落地**之前**量的，属于过期数据。
> 计入修复后实际为 1165，阶段 1–2 净**增加** 35 行。

**行数不降反增。** filter 链接线、「被吞掉」分支的清理逻辑，以及三个复查修复
（说话人查询/注册拆分、`filtered_segment` 重建、文档补全）都在加行。**不要拿
行数当阶段 3–5 的验收指标** —— 结构解耦（加插件不碰核心）已达成，那才是目标。

对照阶段 3：删掉整块 metrics/nudge 调度、没有接线代码抵消，`lib.rs` 由 1165
降至 1125（净减 40 行）。**搬走「调度逻辑」才会瘦身，搬走「判定逻辑」不会** ——
后者只是把代码从核心挪到插件，接线成本还要倒贴。

**过程中发现并修复的两个既存缺陷**（不在本设计范围内，但影响结论可信度）：

1. **测试目录名碰撞导致偶发失败。** `TempDir` 仅用 `SystemTime::now().as_nanos()` 命名，而 macOS 该时钟实际只有微秒分辨率（实测连续 1000 次取值有 956 次相同）。并行测试拿到同一路径，先结束者 `Drop` 删除目录，另一个随即报 `unable to open database file`。已改为 pid + 进程内原子计数器。**此前所有「测试全绿」的结论都含运气成分。**

2. **跨流回声重复段仍触发插件**（§3.3 已记录的既存缺陷）。修复随阶段 2 落地，并新增回归测试 `filtered_segments_never_reach_observers` 守住「被 filter 吞掉的段一次都不许派发到 observer」。该测试经破坏性验证：把 `apply_filters` 挪回 emit 包装后它变红（派发 6 次 vs 存活 3 段），而原有的 `cross_stream_echo_dedup_keeps_single_copy` **在同样破坏下仍为绿** —— 说明它守住的确实是此前无人看守的不变量。

## 8. 架构约束（评审用）

- 插件不得拥有钩子注册表，只能注册进去
- `EventFilter` 不得接触 `PluginContext`、不得做 IO
- 插件自产事件不回灌 filter 链
- finalizer 之间互不阻塞
- 新增插件不得要求修改 pipeline、config 结构体或前端
- 每阶段结束时 `TALKSAGE_REQUIRE_MODELS=1` 下全量测试通过且零跳过
