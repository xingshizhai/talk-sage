# 实时转写功能与相关模式分析

> 对象：talk-sage **v0.2.2**「实时转写」页（语音转写）
> 文档目的：梳理实时转写的数据流、状态机、计时器语义，以及与它耦合的几种
> 「模式」（转写视图模式 / 场景模式 / 引擎与路由 / 载体模式），便于后续维护与扩展。
> 修订：对齐 v0.2.2；补充 §3.4「流式 vs 段级引擎」（决定有没有 partial 的分水岭）、
> §3.5 有界队列与背压、§4.6 会议文字通道、§4.7 实时知识检索，并记录 §6 的实时性优化。

---

## 1. 功能总览

「实时转写」是应用的主页（`navPage === "transcript"`），核心链路：

```
麦克风/系统音频/文件导入/会议文字
  → 音频采集 + 增益 + 重采样 + 降噪 + VAD 端点检测
  → 本地/云端 ASR（流式：逐块出 partial；段级：段结束一次性出文本）
  → 领域事件流（segment / snapshot / level / key_point / term / translation /
                knowledge_query / metrics / nudge / session_stats / media_*）
  → 前端聚合（TranscriptAccumulator）→ 3 种视图渲染
  → 会话落库（SQLite）+ 录音分轨 + master 主录音 + 会后分析（finalizer）
```

### 1.1 三层结构

| 层 | 载体 | 职责 |
|---|---|---|
| 前端 `web/src` | React + Vite | 开始/停止/暂停监听、实时渲染、要点/术语/简报侧栏、会话指标、计时器 |
| 桌面壳 `web/src-tauri` | Tauri 2 (Rust) | IPC 桥接：`start_listen` / `stop_listen` / `set_listen_paused` / `inject_text` / 事件 emit |
| 引擎 `crates/*` | Rust workspace | pipeline（采集→VAD→ASR→事件）、session 存储、config、插件 |

Headless 载体（`talksage serve`）复用同一 pipeline，仅事件传输从 Tauri IPC
换成 WS + HTTP（`transport.ts` 的 `httpApi` 分支）。

### 1.2 事件流（DomainEvent）

前端通过 `api.onEvent` 一次性订阅（App.tsx 的 `useEffect` 只注册一次，内部用
ref 读最新状态，避免闭包过期）。`DomainEvent` 在 `crates/talksage-core/src/lib.rs:104`
定义，共 17 个变体：

| 事件 | 生产者 | 前端消费点 |
|---|---|---|
| `status` | pipeline 各阶段 / 服务层 | App.tsx 状态机（§2.1） |
| `snapshot` | WS 订阅时（`server/lib.rs::handle_ws`） | 重建整个实时上下文（§2.3） |
| `segment` | 两条流的 worker（partial + final） | `TranscriptAccumulator` |
| `level` | 独立电平线程（100ms 节流） | 左侧电平表 |
| `key_point` / `key_point_flush` | `key_point_llm` 插件 | 要点卡片 / 整理回执 |
| `term` | `term_explainer` 插件 | 右侧专业知识卡片 |
| `translation` | `translator` 插件 | 转写行下方译文 |
| `knowledge_query` | `LiveKnowledgeRetriever` | 右侧知识查询记录 |
| `metrics` / `nudge` | `conversation_metrics` 插件 | 指标条 / 会中提示 |
| `session_stats` | pipeline 收尾（每流一条） | 不直接渲染，供质量评估与历史回溯 |
| `media_progress` / `media_completed` | 文件导入路径 | 进度条 / 完成操作栏 |
| `chat_delta` | AI 助手 | ChatSection |
| `model_progress` | 模型下载 | ModelsSection |
| `brief` | `brief_retriever`（**已停用**） | 无（协议兼容保留） |
| `state` | 无生产者（历史遗留） | 无 |

`TranscriptAccumulator`（`lib/transcript.ts`）是纯前端聚合器：partial 阶段按稳定的
输入通道 `speaker_id` 对齐，final 到达后同时保留 `speaker_attribution.voice.id`（会话
内声纹身份）和业务角色。`getLines()` 输出渲染行。**快照与增量事件共用同一个
accumulator**；快照带来 `revision` 基线，之后 revision 不大于基线的事件被丢弃
（§2.3），保证重连既不重复也不丢段。

---

## 2. 监听状态机与计时器

### 2.1 状态机（前端侧）

由 `status` 事件的 `stage` 字段驱动（`StatusStage`：`starting` / `asr_loading` /
`asr_ready` / `recording` / `paused` / `importing` / `idle`）：

```
待机 (idle)
  │  startListen()
  ▼
starting → asr_loading → asr_ready → recording（监听中）
  │                                    │
  │  setListenPaused(true)   setListenPaused(false)
  ▼                                    ▼
paused（暂停）──────────────────► recording（恢复）
  │
  │  stopListen()
  ▼
idle（已停止，落库 + 录音收尾 + finalizer）
```

- `recording` / `importing` / `paused` 三种 stage 映射到 `listening`（+ `paused`）布尔。
- `idle` / `asr_ready` 都视为「非监听」。
- **暂停不是「停止再开始」**：模型与会话都保持存活，`RuntimeParams.paused` 是
  跨线程共享的 atomic，事件循环里检测到翻转就调用各流的 `pause()`
  （提交当前句 → 清 VAD/预滚），随后每轮 `drain_paused()` 丢弃设备在此期间产生的
  音频，避免恢复后识别到暂停期间的积压。

### 2.2 计时器语义

计时核心在 App.tsx 的 `timerRef`：

```ts
{ start: number /* 当前活跃段起点，暂停时置 0 */, accumMs: number /* 暂停前累计 */ }
```

- **开始**（`!prev` 的 recording 事件）：`{ start: now, accumMs: 0 }`。
- **恢复**（`prev` 为真的 recording 事件）：`start = now`，`accumMs` 保留。
- **暂停**：`accumMs += now - start`，`start = 0` → **暂停期间不走表**。
- **停止/idle**：清零。
- 每秒 interval 计算 `accumMs + (now - start)` 更新 `listenElapsed`（秒）。

显示位置：

1. **转写卡片头部右侧**：红点呼吸闪烁 + `H:MM:SS`/`MM:SS` 等宽数字，
   暂停时红点变黄（`--brief`）并冻结。格式函数 `fmtElapsed` 导出自
   `TranscriptCard.tsx`，三处显示（卡片、页头徽章、侧栏「活跃」）共用，保证一致。
2. **页头徽章**：`● 场景 · 时长`（仅监听中）。
3. **侧栏运行状态**：`活跃 MM:SS` / `暂停` / `待机`。

计时语义要点：**计时的是「有效转写时长」**——暂停（开会间隙、临时静音整理）
不计入，恢复后从暂停点续走；停止后归零，新会话重新开始。这与录音机、计时器
的直觉一致，避免「暂停十分钟，转写时长虚高」。

### 2.3 快照重建与 revision 对齐

快照在两种情况下发给界面：headless 每次建立 WS 时（`server/lib.rs::handle_ws`），
以及桌面 WebView 重新挂上事件之后（`listen_snapshot`）。用来在刷新、断线、
锁屏后重建同一场监听。两条不变量：

1. **服务端先订阅广播、再取快照。** 反序（先取快照再订阅）时，"取快照"与
   "订阅"之间提交的段既不在快照里、也不会广播给该客户端，静默永久丢失。
   先订阅的代价是那段窗口的事件会被补发一次。
2. **客户端用 `revision` 去重。** 快照携带 `TranscriptState.revision`，之后
   `revision` 不大于该基线的 segment 事件被 `TranscriptAccumulator` 丢弃 ——
   补发事件因此不会变成重复的转写行。打字段（`speaker_attribution.source ==
   "typed_text"`）豁免：它不经过 `TranscriptState`、`revision` 恒 0、也永远
   不出现在快照里（见 §4.6）。

重连由 `transport.ts` 的 `httpApi.onEvent` 负责：`onclose` 后按指数退避
（0.5s 起，上限 10s）重连；服务端每次建立连接都先发快照，所以重连即完成状态
重建，前端不需要另写补拉逻辑。快照恢复的是**后端真实阶段**（`recording` /
`paused` / `idle`），不只是"是否在监听"——否则前端 `paused=false` 配上后端
`paused=true`，用户点暂停会发出反向动作。

快照带 `elapsed_ms`（这场监听的有效时长，暂停不计入）。重连后计时从这里续上，
不会因为界面被重建而从 0 开始，看起来像新开了一场。旧服务端没有这个字段时仍从 0 计。

---

## 3. 关键机制：从音频到一行文字

### 3.1 采集与预处理

| 步骤 | 位置 | 说明 |
|---|---|---|
| 设备回调 | `talksage-audio/src/lib.rs` | cpal / WASAPI 回调只做：声道选择 → 增益 → 重采样 → 定长分块 → `try_send` |
| 声道选择 | `collect_and_send` | 多声道取 **RMS 最大的单声道**，不混音（无线麦常只有一路有信号，混音会衰减 ~6dB 或反相抵消） |
| 增益 | `AudioHub::new_with_gain` | 0..24dB，输出限幅 ±0.98 |
| 重采样 | `resample_linear` | 线性插值到 16kHz（**已知局限见 §6**） |
| 背景降噪 | `Preprocessor` | 一阶高通 + 块级噪声门；**默认关闭**，因为噪声门会把远端/轻声的弱信号整块静音，导致 VAD 判"无声" |
| 运行时噪声门 | pipeline `tick()` | 侧栏可实时调的 `noise_level`，在 VAD/ASR 之前把低于门槛的块清零；与 `denoise.gate_threshold` 是两套独立门槛 |

Windows 锁屏会使 WASAPI 设备失效。麦克风回调停止后，管道大约 2 秒（或一收到流错误）
就关掉旧流并重新打开默认输入设备。会话不结束，也不另开一场；设备还没回来时每 2 秒再试。
系统回环在没人放声时本来就可能没有数据包，不按断流去重启。

### 3.2 调度与 VAD 分段

- 事件循环是**单线程**的，用 `RoundRobin` 每轮换起始流，避免固定让 user 流优先。
- 文件输入由 `FilePacer` 按 deadline 放行（不在循环里 sleep）；暂停时 `postpone()`
  把 deadline 推到未来，恢复后不会为追赶旧时钟而瞬间灌入大量音频。
  倍速只改变放行节拍，不改变音频时间戳。
- VAD 用 Silero（`threshold` / `min_speech` / `min_silence` / `max_speech` 由预设 +
  覆盖决定，标准档 0.50 / 0.25s / 0.50s / 30s）。
- **500ms pre-roll**：VAD 确认起音前的音频会被回放给 ASR，否则普通话首字极易被截断。
- 声纹场景下每段音频另存一份（≤30s）供说话人识别与滑动窗口换人检测。

### 3.3 端点判定

`endpoint.rs` 的 `StableEndpoint` 决定「当前语义段能否提交」。提交必须
**同时**满足：

- 文本足够长：`min_segment_ms`（默认 1000ms）；
- 且（文本 hypothesis 连续不变 ≥ `stable_ms`（350ms）**并且** 音频安静
  ≥ `quiet_ms`（450ms，`quiet_rms` 0.008））
  **或** 强制静音 ≥ `force_quiet_ms`（850ms）。

即「文本稳定 + 短暂停顿」双条件，另加"文本一直在变但确实没人说话了"的兜底。
`segment.rs::SegmentLifecycle` 把 VAD 段尾与主动端点两种信号合并成
`EndpointDecision { commit, natural }`：主动端点提交后会清空 VAD 内部状态，
避免旧段尾立刻切断下一句。

### 3.4 流式 vs 段级引擎 —— 有没有 partial 的分水岭

这是理解"实时感"的关键。**增量文本只来自 `engine.accept() -> Some`**：

| 引擎 | `is_streaming()` | partial（逐块上屏） | 段的形成方式 |
|---|---|---|---|
| `paraformer-zh`（sherpa 流式） | ✅ | ✅ 有 | 流式 hypothesis + 端点判定 |
| `zipformer-en`（sherpa 流式） | ✅ | ✅ 有 | 同上 |
| `aliyun-cloud` | ✅ | ✅ 有 | 云端回传 partial / SentenceEnd |
| `qwen3-asr`（sherpa 离线） | ❌ | ❌ 无 | VAD 段尾 / 强制切段 |
| `whisper-base` / `whisper-small` | ❌ | ❌ 无 | 同上 |
| `whisper-medium-metal` | ❌ | ❌ 无 | 同上 |
| `whisper-large-v3-turbo-metal` | ❌ | ❌ 无 | 同上 |

`WhisperMetalEngine::accept` 只往缓冲区 append 并返回 `None`，全文只在 `finish()`
产出（`crates/talksage-asr/src/metal.rs`）。而 Windows Vulkan / macOS Metal 路由下
`service.rs` 会**默认把引擎改写成 `whisper-large-v3-turbo-metal`**，只有用户显式选了
某个 whisper 模型或走 CLI `--engine` 才不覆盖。

因此：

- 产品默认 GPU 路径的"实时"是**段粒度**（VAD 段尾，或 `force_segment_ms` 到点），
  不是逐字上屏。观感上的延迟 ≈ 段长 + 一次推理耗时。
- 段长由场景的 `asr_segment_ms` 决定（见 §4.2），且只对**非流式**引擎生效
  （`force_segment_ms = if user_engine.is_streaming() { 0 } else { scene.asr_segment_ms }`）。
- 强制切分发生在当前音频块**送入 ASR 之后**（旧实现在 finish 前 return，会整块丢掉
  切点附近的音频，表现为句尾字消失）。
- 想获得逐字上屏，需要切到流式引擎（Paraformer/Zipformer）或阿里云云端模式。

### 3.5 有界队列与背压

"队列有界"是这套系统能长时间跑的前提。全部边界：

| 边界 | 容量 | 满时行为 | 代码 |
|---|---|---|---|
| 采集 → StreamWorker | 512 帧 ≈ 51s | 丢帧 + 记 overrun + warn（不阻塞音频回调） | `talksage-audio/src/lib.rs` |
| StreamWorker → ASR（音频） | `ASR_AUDIO_QUEUE_CAP = 128` ≈ 12.8s | 丢块 + 记 overrun + warn | `pipeline/lib.rs` |
| StreamWorker → ASR（控制） | 同上，但走阻塞 `send` | 短暂背压（必须保序送达） | `pipeline/lib.rs` |
| ASR → StreamWorker（结果） | 无界 | 结构小、段级一条，无需设限 | `pipeline/lib.rs` |
| durable 事件 → `SessionWriter` | 256 | **阻塞**（宁可减速也不静默丢会话数据） | `pipeline/session_writer.rs` |
| 慢插件任务 → `PluginExecutor` | 4 worker × 64 | 丢任务 + warn + 回调插件撤销骨架 | `pipeline/plugin_executor.rs` |
| 声纹换人检测 | 1 | 跳过本窗口，不追赶历史 | `pipeline/speaker_change.rs` |

设计取舍：**快路径（音频）宁可丢帧也不阻塞、也不无限涨内存；慢路径（LLM）丢任务
并让插件自己收尾；只有会话持久化选择阻塞**（它是唯一"丢了就补不回来"的数据）。
采集/ASR 的丢弃都会在停止时汇总告警，因为它们是"转写为什么缺字"的唯一直接证据。

### 3.6 ASR 线程化

引擎与标点模型都住在每条流自己的 `talksage-asr-{label}` 线程上
（`AsrChannel`）。之前它们在采集/VAD 那条线程上同步跑：whisper 段级推理一次
2~3s，期间转写不出字、另一条流也被拖住，界面表现为"卡一下、再一批吐出来"。

两个要点：

- **段代号（generation）**：每次收尾自增，切段后才回来的旧 partial 被丢弃
  （否则上一句的残文会污染新段）。
- **收尾必须等**：`finish_and_wait` 先 `finish_speech` 再 `drain_asr_blocking(10s)`，
  否则最后一句话会丢。暂停同样走这条路（`pause()` → `finish_and_wait`）。

### 3.7 发段与插件

`emit_final` 的处理顺序（每一步都有理由）：

1. **filter 链在产生点施加**：被吞掉的事件既不 emit、也不触发 observer，也不进
   统计 —— 短段抑制和跨流去重要同时拦住这三者。
2. **filter 是变换而非仅丢弃**：统计与 observer 都读 filter **之后**的段
   （`filtered_segment`），否则第一个做改写（脱敏/规范化）的 filter 一上线，
   落库文本就会和插件、词数统计静默错位。
3. **说话人分配延迟 commit**：只有 filter 放行后才 `commit()`，所以被短段抑制或
   跨流去重吞掉的段不会污染声纹聚类库。
4. **骨架同步、慢路径异步**：`skeleton()` 是本地即时（无 HTTP），`run()` 交给会话级
   `PluginExecutor`。
5. **队列满时回调插件**：`submit_rejected()` 让插件撤销已经发出的骨架
   （例如 `term_explainer` 收掉「正在整理专业知识…」卡片），否则界面会永远停在中间态。

### 3.8 说话人归属

- 策略是显式的 `off / channel / voiceprint`（`speaker_mode`），不是布尔开关。
- **`speaker_id` = 稳定输入通道**（0 我 / 1 客户 / 2 文字），用于 partial↔final 对齐、
  翻译方向与统计分组；**`speaker_attribution.voice.id` = 会话内声纹身份**（复用
  声纹编号不会覆盖通道身份，避免 A/B 交替发言互相覆盖）。
- 会议模式（`voiceprint`）两条流都参与聚类，但**聚类中心按角色域隔离**：麦克风产生
  「讲话者N」，回环产生「客户N」；已注册的主人声纹只在麦克风流匹配，避免本机回声
  把远端段误标为"我"。
- **编号按角色域独立**：两个域各自从 1 开始（`RoleDomain`），共用一条序列会让双流
  交错时出现「讲话者1 / 客户2」这类跳号。编号在**查询时**以无锁原子方式预留（两条
  ASR 线程可能并发查询，延迟分配会撞号），注册仍延迟到 filter 放行后的 `commit`；
  代价见 §6.2 第 11 条。候选以标签为键（标签在域内唯一），因此确认前后显示身份不变。
- 滑动窗口换人检测（1.5s 窗口 / 0.5s 步进 / 连续 2 个窗口偏离 / 至少 2s 轮次）
  可以在长 VAD 段中途安全切分，而 partial 文本不受影响；推理跑在容量为 1 的独立
  队列上，不会给实时 ASR 制造积压。

---

## 4. 相关模式

### 4.1 转写视图模式（TranscriptMode）

`TranscriptCard` 的三种渲染模式，通过 `lib/prefs.ts` 持久化：

| 模式 | 渲染 | 适用 |
|---|---|---|
| `timeline` 时间线 | 每段独立行 + 左侧时间戳 + 说话人/引擎 | 默认，逐句核对 |
| `focus` 专注 | 最后一段放大高亮，其余降透明 | 盯当前句 |
| `dense` 密集 | 连续文字流，句间用 `｜` 分隔 | 快速通读 |

关键实现：`punctuateAndSplit`（`lib/transcript.ts`）先 `smartPunctuate`
（疑问尾词 → ？、连词/主语词处断句、长度兜底补 ，）再 `splitSentences` 切句；
`｜` 是前端句间分隔符（与后端存储文本无关，纯展示层）。

### 4.2 场景模式（SceneMode）与数据化场景模板

配置 `[scene] mode`，决定**整场会议的管道装配**（不是纯 UI 开关）：

| 模式 | 流 | 客户流 | `asr_segment_ms` | `min_segment_ms` | 说话人 | 插件 allowlist |
|---|---|---|---|---|---|---|
| `dictation` 单人听写 | 单流 | ✗ | 5000 | 0 | off | key_point_llm |
| `conversation` 一对一会话（默认） | 双流 | ✓ | 4000 | 300 | channel | term / brief* / key_point |
| `bilingual` 双语对话 | 双流 + 翻译 | ✓ | 4000 | 300 | channel | 全部 |
| `live_translation` 实时翻译 | 单流 | ✗ | 3000 | 300 | off | translator |
| `meeting` 多人会议 | 双流 + 声纹 | ✓ | 6000 | 0 | **voiceprint** | 全部 |
| `lecture` 演讲/课堂 | 单流为主 | ✗ | 6000 | 300 | off | term / brief* / key_point |
| `custom` 自定义 | 由 `[scene.custom]` 裁决 | ✓ | 4000 | 0 | channel | 全部 |

\* `brief_retriever` 已被宿主**无条件停用**（实时检索改由 §4.7 的
`LiveKnowledgeRetriever` 编排），allowlist 里的条目是历史遗留。

要点：

- 场景通过 `plugin_overrides_for`（`pipeline/service.rs`）裁决插件启用与参数。
  合并顺序为 `插件默认值 → 用户 [plugins.<id>] → 宿主/场景裁决`；分析类插件用
  **allowlist 单向关闭**（新增插件不会因为某个场景忘了更新而意外开启）。
- `asr_segment_ms` 只对非流式引擎生效（§3.4），它是段级引擎的延迟旋钮。
- 要点聚合的批量窗口也跟场景走（`key_point_aggregation_policy`）：实时翻译 4 段/15s、
  会话与双语 6/30s、听写 8/45s、会议与课堂 12/60s。
- **场景切换 = 重新装配会话**，因此设置页有未保存改动时离开会弹确认
  （防带着旧场景开始监听）。
- **数据化场景模板**："面试复盘 / 客服质检 / 客户陪访"是配置定义的模板
  （基底场景 + 参数覆盖），可在 `talksage.toml` 的 `[scene] presets` 增补，
  **不需要改 Rust 代码**；套用后进入完全可编辑的自定义场景。

### 4.3 引擎与路由模式

`[asr]` 配置 + `EngineKind`（`crates/talksage-asr`）：

- **引擎（8 个变体）**：产品可选 3 个 —— `qwen3-asr`（sherpa-onnx 离线段级 int8）、
  `whisper-medium-metal`、`whisper-large-v3-turbo-metal`（whisper.cpp GPU：
  macOS Metal / Windows Vulkan）；另有 5 个非产品目录变体用于诊断/回归
  （流式 `paraformer-zh` / `zipformer-en`、离线 `whisper-base` / `whisper-small`、
  云端 `aliyun-cloud`）。
- **路由**：`asr_mode = auto | local | cloud` + `backend = auto | cpu | cuda | metal | vulkan`。
  `auto` 的判定是"有加速后端就用本地 GPU，否则凭据完整就用阿里云，否则报错"；
  显式 `backend` 只在与探测结果一致时接受，否则给出可读错误（`intel/openvino` 明确未实现）。
- **能力探测**：`physical_gpu` / `runtime_backend` / `effective_route` 在启动时打日志，
  并显示在侧栏（如 `Vulkan GPU (whisper.cpp)`）。`availability_note()` 专门解释
  "物理显卡存在但运行期后端不可用"这类不一致。
- **引擎池**：引擎按 `{display_name}|{model_dir}|{hotword签名}|provider|language` 常驻复用
  （监听热启动）。**已知局限见 §6**。
- 云端模式（阿里云 ASR）在 CLI `listen` 入口不支持（需 Tokio runtime），
  桌面/headless 服务正常。

### 4.4 载体模式（transport）

`transport.ts` 的 `getApi()` 按环境返回：

- **Tauri IPC**（`ipcApi`）：`invoke` + `convertFileSrc`（录音 URL 用完整路径）。
- **Headless HTTP+WS**（`httpApi`）：`/api/*` REST + WS 事件流（断线指数退避重连），
  录音 URL 用纯文件名经 `/api/recordings/<name>`（server 端自动扫描新旧目录布局）。

两侧实现同一 `AppApi` 接口，业务层（App.tsx）零分支。

**桌面 vs headless 的实时性差异**：headless 在每次 WS 连接时由服务端先推快照。
桌面没有这条连接，但 WebView 在锁屏、休眠或显卡进程退出后会被重建，React 状态
回到「开始监听」。桌面因此在订阅 `talksage://event` 之后立刻调用 `listen_snapshot`，
把同一场的阶段、转写和已计时长补回来。两边都用 §2.3 的 revision 对齐去掉补发的重复段。

### 4.5 实时输入与文件输入统一会话

- 麦克风、系统回环与导入媒体均写入 `AppState.running`，使用同一个 `talksage://event`
  事件流；前端只有一套 transcript / term / key-point / translation 状态。
- 文件会话启动命令立即返回 `session_id`，后台在 EOF 或用户停止后统一调用
  `TalkSageService::finish`，随后发出 `media_completed`。界面不会自动跳到历史页。
- `media_progress` 以音频采样位置驱动时间和进度，不使用墙钟；1×/2×/4×/极速只改变
  文件块调度节拍，不改变音频时间戳。暂停时进度与计时同时冻结。
- 导入文件也生成会话主录音，因此历史回放、导出和质量评估与实时监听一致。
- WAV / MP3 / MP4 / M4A 本地解码 → mono → 16kHz → 与实时同一条管道。

### 4.6 会议文字通道

实时监听中可在转写页顶部输入文字（远程参会者的提问/补充），Enter 发送即并入转写流：

- 走 `RunningListen::inject_text` → **与语音段共用同一条事件 sink**，因此会经术语去重、
  知识检索观察器、SQLite 落库、IPC/WS 广播 —— 展示与入库只有一套路径。
- 来源记为 `AudioSource::TypedText`，`duration_ms` / `rms` 为 0，不计入语音时长类指标。
- 保留说话人通道号 `speaker_id = 2`（与语音的 0/1 错开，避免 partial/final 对齐键冲突）；
  单条上限 5000 字符，空文本拒绝。
- **它不经过 `TranscriptState`**（`SessionRuntime` 只包装 Segment 事件），所以：
  - 不出现在 `snapshot` 里 —— 重连后实时视图只显示语音段（打字段在 History 中完整可见）；
  - `revision` 恒为 0 —— 因此 §2.3 的 revision 门槛必须豁免它，否则快照一到，
    后续所有打字段都会被当成"已过期"丢掉。
- 指标侧的影响：`compute_conversation_metrics` 把 `speaker_id != 0` 一律归为"对方"，
  于是打字会计入对方的段数/词数，并因 `duration_ms == 0` 拉低对方平均段长
  （见 §6 遗留项）。

### 4.7 实时知识库查询

知识库的「文档已被索引」与「本次会话已查询」是两件事。未开始实时转写时，
右侧栏只显示材料包数量和待机说明，不把文档目录或置顶正文当成检索结果。

会话启动后，`LiveKnowledgeRetriever` 仅在以下语义节点触发查询：

1. `key_point` 到达 `final`：用要点类别和内容检索。
2. `term` 到达 `final`：优先提取「术语：解释」中的术语名检索。
3. 定稿 `segment` 是明确问句：含问号，或「是什么 / 为什么 / 怎么 / 多少 / 什么时候 /
   是否 / 能否 / 哪一个」等标记，且长度 ≥ 8 字。

不再对每条普通转写按冷却时间盲目检索；旧 `brief_retriever` 保留为协议兼容实现，
运行时由宿主关闭，设置页不再允许单独开启。

检索顺序与界面语义：

- 有置顶材料时先只查材料包；材料包无命中才回退到全库。
- 无置顶材料时直接查全库，每次最多返回 3 个片段。
- 查询在会话内按归一化文本去重，相同要点或术语不会重复搜索。
- 右侧栏最多保留 20 条「已查询」记录，展示触发类型、查询词、置顶/全库范围、
  来源路径、标题、相关度和摘要；无命中也保留查询记录。

### 4.8 实时智能（要点 / 术语 / 翻译 / 指标 / 提示）

| 能力 | 插件 | 触发 | LLM 成本 |
|---|---|---|---|
| 要点聚合 | `key_point_llm` | 缓冲区满 `batch_size`（场景决定）或尾部超时或手动「立即整理」 | 每批次 1 次 |
| 专业知识 | `term_explainer` | 用户关注的知识点命中（不受冷却），或 LLM 从会话里整理术语和知识点（受冷却 + 最短字数 + 专业度门限）；也可以在右栏输入一个没听懂的知识点当场讲解 | 自动整理每次触发 1 次；手动讲解另计 |
| 实时翻译 | `translator` | 每个 final 段（方向由场景 translation_mode + 通道决定） | 每段 1 次（双向每流 1 次） |
| 会话指标 | `conversation_metrics` | 每个 final 段（纯本地统计） | 无 |
| 会中提示 | 同上（`nudge_enabled`，默认关） | 规则 + 2 分钟冷却 | 无 |
| 会话质量 | `session_quality`（finalizer） | 会后一次 | 无 |
| webhook | `webhook`（finalizer，默认关） | 会后一次，SSRF 保护 | 无 |

注意 **`translator` 没有冷却**（配置里的 `cooldown_seconds` 是有意不读的）：实时翻译
的语义就是"每句都要译"，加冷却会丢句子。它的成本压力由 `PluginExecutor` 的
worker 数（4）承担，见 §6。

---

## 5. 关键代码位置

| 关注点 | 位置 |
|---|---|
| 状态机 + 计时器 + 事件订阅 + 快照恢复 | `web/src/App.tsx` |
| 计时器 UI + `fmtElapsed` | `web/src/components/TranscriptCard.tsx` |
| 视图模式/分句 | `web/src/components/TranscriptCard.tsx`、`web/src/lib/transcript.ts` |
| 聚合器 + revision 去重 | `web/src/lib/transcript.ts` |
| 双载体 API + WS 重连 | `web/src/lib/transport.ts` |
| 事件类型 | `web/src/lib/api.ts`（DomainEvent） |
| 管道编排（事件循环 / ASR 线程 / 发段） | `crates/talksage-pipeline/src/lib.rs` |
| 端点判定 / 段生命周期 | `crates/talksage-pipeline/src/endpoint.rs`、`segment.rs` |
| 输入轮询与文件节拍 | `crates/talksage-pipeline/src/input_scheduler.rs` |
| 会话装配 / 场景裁决 | `crates/talksage-pipeline/src/service.rs` |
| 慢插件执行器 | `crates/talksage-pipeline/src/plugin_executor.rs` |
| 会话持久化写线程 | `crates/talksage-pipeline/src/session_writer.rs` |
| 说话人归属 / 换人检测 | `crates/talksage-pipeline/src/speaker*.rs` |
| 引擎枚举 / 路由 / 引擎池 | `crates/talksage-asr/src/lib.rs`、`routing.rs`、`gpu.rs` |
| 引擎适配器 | `crates/talksage-asr/src/metal.rs`（whisper.cpp）、`aliyun/` |
| 实时知识检索 | `crates/talksage-pipeline/src/knowledge.rs` |
| 插件注册表 / 钩子契约 | `crates/talksage-plugins/src/registry.rs`、`builtin.rs` |
| WS 订阅与快照下发 | `crates/talksage-server/src/lib.rs`（`handle_ws`） |
| 采集 / 预处理 / 重采样 | `crates/talksage-audio/src/lib.rs` |

---

## 6. 已知限制与后续项

### 6.1 已在本轮修复

| 问题 | 修复 |
|---|---|
| worker → ASR 音频通道**无界**：推理慢于实时时内存无上限增长，而采集侧同时在丢帧 | 改为 `sync_channel(ASR_AUDIO_QUEUE_CAP = 128)`，音频走 `try_send` 丢弃并计数，控制命令仍阻塞保序；停止时汇总告警（`pipeline/lib.rs`） |
| 慢插件任务被丢弃时**骨架卡片永久停留**在"识别中…"（`run` 不会被调用） | 新增 `SegmentObserver::submit_rejected`，段路径在 `submit` 失败时回调插件撤销骨架；`term_explainer` 已实现 |
| `PluginExecutor` 只有 2 个 worker，并发插件数 ≥ 3 时互相饿死 | 提升为 4 worker × 64 队列 |
| `key_point_llm` 的空转轮询：已有任务在跑时每 250ms 仍预留并投递一个必然立即返回的 idle 任务，把有界队列挤满 | `idle_trigger_due()` 增加 `!llm_busy` 守卫 |
| headless WS **断开后静默停更、永不恢复** | 前端 `onclose` 指数退避重连；服务端改为"先订阅广播、后取快照"消除丢事件窗口；客户端按 `revision` 去重消除补发重复 |
| 快照刷新后**暂停态丢失**，点暂停会发出反向动作 | 快照按 `stage` 恢复 `recording` / `paused` / `idle` 三态 |
| `FilePacer::consumed` 用 `Duration::div_f32`（经 f32 秒回转），2× 的节拍比半间隔晚 1ns，测试与"倍速精确对齐"断言失败 | 改为整数纳秒四舍五入 |
| `scene_allowlist_controls_each_preset` 与宿主停用 `brief_retriever` 的现实不符（测试红） | 测试显式声明宿主停用集合，其余插件仍严格跟随 allowlist |
| 说话人编号用**一条全局序列**，而标签前缀按角色域区分 → 双流交错时出现「讲话者1 / 客户2」这类跳号（看到"客户2"却没有"客户1"），与"麦克风 讲话者N / 回环 客户N"的承诺不符 | 改为**每角色域独立计数器**（`RoleDomain`：两个域各自从 1 开始）；`candidates` 改以标签为键（标签在域内唯一），编号不再需要全局互斥 |
| `tests/speaker_live.rs` 两个用例仍断言 d3bcde4 之前的编号语义（"首次只给通用标签、第二次才编号"），会话前就红 | 对齐到当前契约（查询时即编号、确认后显示身份不变），并保留其安全意图；新增 `speaker_numbering_is_independent_per_role_domain` 锁定按域编号与跨域隔离 |

### 6.2 仍存在的限制

1. **默认 GPU 路径没有 partial**（§3.4）：Windows Vulkan / macOS Metal 默认
   whisper.cpp 段级引擎，观感是 3–6s 一批文字。README 已按引擎区分表述；
   想要逐字上屏要用流式引擎或云端。
2. **`SessionWriter` 的阻塞背压直达音频循环**：`emit` 在事件循环线程上同步调用
   `SyncSender::send`（容量 256）。这是刻意取舍（会话数据不可丢），但磁盘/DB 长抖动
   会经由 512 帧采集队列表现为丢音频。若要改，需要把"durable 事件"改成可降级的
   队列 + 溢出告警。
3. **`conversation_metrics` 是 O(n²)**：每个 final 段都对整段历史重新 `collect + sort +
   计词`，并把 `Metrics` 事件广播一次。数千段的长会议下这是主循环上的持续开销，
   应改为增量累加。
4. **引擎池会随热词变化膨胀**：池键包含热词列表，换会议/改术语表就是一个新键，
   离线引擎重新加载 ~547MiB 模型（首段延迟数秒），且没有全局上限 / TTL / LRU。
5. **线性重采样无抗混叠、也无跨缓冲相位**：`resample_linear` 是纯线性插值；
   `LinearResampler` 每个回调独立调用，非整数比率（如 44.1k→16k = 2.75625）每块
   `floor()` 丢掉小数余量。影响是**音频时间轴与墙钟缓慢漂移**（录音与转写彼此自洽，
   但前端计时器走墙钟、转写行时间戳走音频钟，两者会渐行渐远）。48kHz（比率恰为 3）
   不受影响，而它是 Windows 默认，所以这是条件性缺陷。建议补一条 44.1k 的
   漂移回归测试量化后再动实现。
6. **打字段污染会话指标**：`speaker_id = 2` 被归入"对方"，且 `duration_ms == 0`
   会拉低对方平均段长、并可能被算作一次"打断"。指标应按
   `speaker_attribution.source` 分区，或直接排除 `TypedText`。
7. **全会话增长的状态**：`key_point_llm.seen` 指纹、`TranscriptState.committed`
   （快照会全量 clone）、离线/whisper 引擎的 buffer（仅靠 `force_segment_ms` 兜底，
   为 0 则无上限）。前端每来一个事件都全量重建 `lines`，且 `TranscriptCard` 每行
   每渲染都跑 `punctuateAndSplit`（无 memo），长会话下渲染成本随行数线性增长。
8. **Tauri 命令在启动期间抢锁**：`start_listen` 持 `running` 锁跨越模型加载（数秒），
   而 `set_listen_paused` / `flush_key_points` 是**同步**命令且直接 `lock()` ——
   启动的数秒内点暂停/停止会卡住 UI 线程。
9. **无版本化 delta / 序号 resync**：`revision` 只用于快照去重，不做缺口检测。
   `docs/architecture-v2.md` §19.8 把"版本化 Delta + 序号 resync"列为明确不做；
   当前靠"重连即重新快照"覆盖了这一需求。
10. **静默失败**：whisper 推理失败只打日志、该段变空文本；标点模型加载失败静默
    返回 `None`；`service.finish` 里 `writer.finish()?` 早于 `run_finalizers`，
    writer join 失败会跳过质量评估与 webhook。
11. **被 filter 吞掉的段会消耗本域编号**：说话人编号在**查询时**原子预留（两条 ASR
    线程可能同时查询，编号不能延迟分配），因此"查了但没 commit"的段会占掉本域一个
    编号，表现为编号跳号（如只有「客户1」「客户3」）。这是刻意的取舍：无锁预留的
    编号无法回收。受影响的是被 `cross_stream_dedup` 吞掉的回声段；会议场景
    `min_segment_ms = 0`，短段抑制不会触发。若要消除跳号，只能放弃"查询即编号"，
    代价是同一段在确认前后显示身份会跳变（更差）或双流撞号（不可接受）。
12. **`tests/pipeline_live.rs` 在 CPU 争用下会偶发失败（既有问题）**：
    `file_input_partial_events_precede_final` 要求至少出现一条 partial，但 11 个
    真实模型用例在同一个测试二进制里并行跑会打满 CPU，ASR 线程可能落后一整拍，
    于是该段的 partial 在 `finish_speech` 递增 `asr_generation` 之后才产出、被
    「丢弃过期 partial」逻辑正确丢掉 —— 只剩 final。实测：单独跑 3/3 通过、
    整包连跑 2 次 11/11 通过、但曾在整包运行中失败 1 次。修法是让真实模型用例
    串行（`--test-threads=1`）而不是放宽断言（断言本身是有效的回归网）。
