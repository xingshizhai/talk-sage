# 实时转写功能与相关模式分析

> 对象：talk-sage v0.1.3「实时转写」页（语音转写）
> 文档目的：梳理实时转写的数据流、状态机、计时器语义，以及与它耦合的几种
> 「模式」（转写视图模式 / 场景模式 / 引擎与路由 / 载体模式），便于后续维护与扩展。

---

## 1. 功能总览

「实时转写」是应用的主页（`navPage === "transcript"`），核心链路：

```
麦克风/系统音频/文件导入
  → 音频采集 + 增益 + 降噪 + VAD 端点检测
  → 本地/云端 ASR 流式识别（可双流：用户流 + 客户流）
  → 领域事件流（segment / snapshot / level / key_point / term / translation / brief / metrics / nudge）
  → 前端聚合（TranscriptAccumulator）→ 3 种视图渲染
  → 会话落库（SQLite）+ 录音分轨 + master 主录音 + 会后分析（finalizer）
```

### 1.1 三层结构

| 层 | 载体 | 职责 |
|---|---|---|
| 前端 `web/src` | React + Vite | 开始/停止/暂停监听、实时渲染、要点/术语/简报侧栏、会话指标、计时器 |
| 桌面壳 `web/src-tauri` | Tauri 2 (Rust) | IPC 桥接：`start_listen` / `stop_listen` / `set_listen_paused` / 事件 emit |
| 引擎 `crates/*` | Rust workspace | pipeline（采集→VAD→ASR→事件）、session 存储、config、插件 |

Headless 载体（`talksage serve`）复用同一 pipeline，仅事件传输从 Tauri IPC
换成 WS + HTTP（`transport.ts` 的 `httpApi` 分支）。

### 1.2 事件流（DomainEvent）

前端通过 `api.onEvent` 一次性订阅（App.tsx 的 `useEffect` 只注册一次，内部用
ref 读最新状态，避免闭包过期）。事件类型：

- `status`：状态机迁移（见 §2），前端据此驱动监听/暂停/计时。
- `snapshot`：**全量快照**（committed 已定稿段 + hypothesis 候选段）——连上后端时重建界面用。
- `segment`：单个段完成（含说话人、文本、时间戳）。
- `level`：麦克风/回环电平（驱动侧栏电平条）。
- `key_point` / `key_point_flush`：实时要点与「整理」命令回执。
- `term` / `translation` / `brief`：术语卡片、翻译、简报。
- `metrics`：会话指标（发言占比/语速/提问/独白/打断/健康分）。
- `nudge`：会中提示。

`TranscriptAccumulator`（`lib/transcript.ts`）是纯前端聚合器：按 speaker_id 归并
partial/committed 段，`getLines()` 输出渲染行。**快照与增量事件共用同一个
accumulator**，保证重连/首帧与滚动追加不重复。

---

## 2. 监听状态机与计时器

### 2.1 状态机（前端侧）

由 `status` 事件的 `stage` 字段驱动：

```
待机 (idle)
  │  startListen()
  ▼
启动中 → recording（监听中）
  │              │
  │  setListenPaused(true)     setListenPaused(false)
  ▼              ▼
paused（暂停）───► recording（恢复）
  │
  │  stopListen()
  ▼
idle（已停止，落库 + 录音收尾 + finalizer）
```

- `recording` / `paused` / `idle` / `asr_ready` 四种 stage 映射到
  `listening` / `paused` 两个布尔。
- 停止后进入 `idle` 或 `asr_ready`（模型就绪未监听），两者都视为「非监听」。

### 2.2 计时器语义（v0.1.3 增强）

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

1. **转写卡片头部右侧**（本次新增）：红点呼吸闪烁 + `H:MM:SS`/`MM:SS` 等宽数字，
   暂停时红点变黄（`--brief`）并冻结。格式函数 `fmtElapsed` 导出自
   `TranscriptCard.tsx`，三处显示（卡片、页头徽章、侧栏「活跃」）共用，保证一致。
2. **页头徽章**：`● 场景 · 时长`（仅监听中）。
3. **侧栏运行状态**：`活跃 MM:SS` / `暂停` / `待机`。

计时语义要点：**计时的是「有效转写时长」**——暂停（开会间隙、临时静音整理）
不计入，恢复后从暂停点续走；停止后归零，新会话重新开始。这与录音机、计时器
的直觉一致，避免「暂停十分钟，转写时长虚高」。

---

## 3. 相关模式

### 3.1 转写视图模式（TranscriptMode）

`TranscriptCard` 的三种渲染模式，通过 `lib/prefs.ts` 持久化：

| 模式 | 渲染 | 适用 |
|---|---|---|
| `timeline` 时间线 | 每段独立行 + 左侧时间戳 + 说话人/引擎 | 默认，逐句核对 |
| `focus` 专注 | 最后一段放大高亮，其余降透明 | 盯当前句 |
| `dense` 密集 | 连续文字流，句间用 `｜` 分隔 | 快速通读 |

关键实现：`punctuateAndSplit`（`lib/transcript.ts`）按标点把整段切成句，
`｜` 是前端句间分隔符（与后端存储文本无关，纯展示层）。

### 3.2 场景模式（SceneMode）

配置 `[scene] mode`，决定**整场会议的管道装配**（不是纯 UI 开关）：

| 模式 | 说明 | 主要影响 |
|---|---|---|
| `dictation` 单人听写 | 单流 | 客户流关闭、插件白名单收窄 |
| `conversation` 一对一会话 | 双流（我/对方） | 用户流 + 客户流、说话人 = 通道 |
| `bilingual` 双语对话 | 双流 + 翻译 | 启用 translator 插件、翻译策略 |
| `live_translation` 实时翻译 | 同声传译 | 翻译优先、可能关闭要点 |
| `meeting` 多人会议 | 双流 + 声纹 | speaker 模式、wespeaker 分离 |
| `lecture` 演讲/课堂 | 单流为主 | 长段 VAD、术语/要点侧重 |
| `custom` 自定义 | 完全由 `[scene.custom]` 裁决 | engine/VAD/插件白名单逐项覆盖 |

场景通过 `plugin_overrides_for`（pipeline service.rs）裁决插件启用与参数，
**场景切换 = 重新装配会话**，因此设置页离开时若未保存会弹确认（防带着旧场景
开始监听）。

### 3.3 引擎与路由模式

`[asr]` 配置 + `EngineKind`（talksage-asr）：

- **引擎**：`qwen3-asr`（本地 sherpa-onnx int8）、`whisper-medium-metal`
  （whisper.cpp GPU：macOS Metal / Windows Vulkan）、`whisper-large-v3-turbo-metal`。
- **路由**：`asr_mode = local | cloud` + `backend = auto`。ASR 能力探测决定
  `physical_gpu` / `runtime_backend` / `effective_route`，启动时打日志并在侧栏
  显示（如 `Vulkan GPU (whisper.cpp)`）。
- 云端模式（阿里云 ASR）在 CLI `listen` 入口不支持（需 Tokio runtime），
  桌面/headless 服务正常。

### 3.4 载体模式（transport）

`transport.ts` 的 `getApi()` 按环境返回：

- **Tauri IPC**（`ipcApi`）：`invoke` + `convertFileSrc`（录音 URL 用完整路径）。
- **Headless HTTP+WS**（`httpApi`）：`/api/*` REST + 事件轮询/WS，录音 URL 用
  纯文件名经 `/api/recordings/<name>`（server 端自动扫描新旧目录布局）。

两侧实现同一 `AppApi` 接口，业务层（App.tsx）零分支。

### 3.5 实时输入与文件输入统一会话

- 麦克风和导入媒体均写入 `AppState.running`，使用同一个 `talksage://event` 事件流；
  前端只有一套 transcript / term / key-point / translation 状态。
- 文件会话启动命令立即返回 `session_id`，后台在 EOF 或用户停止后统一调用
  `TalkSageService::finish`，随后发出 `media_completed`。界面不会自动跳到历史页。
- `media_progress` 以音频采样位置驱动时间和进度，不使用墙钟；1×/2×/4×/极速只改变
  文件块调度节拍，不改变音频时间戳。暂停时进度与计时同时冻结。
- 导入文件也生成会话主录音，因此历史回放、导出和质量评估与实时监听一致。

---

## 4. 关键代码位置

| 关注点 | 位置 |
|---|---|
| 状态机 + 计时器 + 事件订阅 | `web/src/App.tsx` |
| 计时器 UI + `fmtElapsed` | `web/src/components/TranscriptCard.tsx` |
| 视图模式/分句 | `web/src/components/TranscriptCard.tsx`、`web/src/lib/transcript.ts` |
| 聚合器 | `web/src/lib/transcript.ts`（`TranscriptAccumulator`） |
| 双载体 API | `web/src/lib/transport.ts` |
| 会话装配/场景裁决 | `crates/talksage-pipeline/src/service.rs` |
| 引擎枚举/探测 | `crates/talksage-asr/src/lib.rs` |
| 事件类型 | `web/src/lib/api.ts`（DomainEvent） |
