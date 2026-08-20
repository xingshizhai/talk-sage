# 参考项目研究：Call.md（video-db/call.md）

> 调研日期：2026-08-20。来源：`D:\Work\github\call.md`（VideoDB 开源，Electron + TypeScript，云端转写）。

## 一、项目定位

**把会议变成"实时智能体回路"（live agent loop）的桌面应用**：本地录制（麦克风 + 系统音频双通道）→ 云端 VideoDB 实时转写（WS）→ 会中实时智能（指标/提示/assist/自动调工具）→ 会后三段式纪要 + Webhook 自动化。

- 定位对比：Call.md = **销售/商务会议场景**的云端智能助手（需要 API key、联网），UI 精致（shadcn/ui + 小组件窗口）；
  talk-sage = **本地优先**的通用会议助理（完全离线 ASR）。两者产品形态高度相似（双通道转写、历史、设置、托盘），但 Call.md 的"会中实时智能"（指标、coaching、live assist、MCP）明显更丰富，是主要借鉴来源。

## 二、技术栈对比

| 维度 | Call.md | talk-sage | 备注 |
|---|---|---|---|
| 桌面壳 | Electron 42 | Tauri 2 | — |
| 前端 | React 19 + Tailwind + shadcn/ui + Zustand | React/Vite/TS（手写样式） | UI 组件库可参考 |
| IPC/API | tRPC 11 + Hono（主↔渲染进程端到端类型安全） | Tauri command（serde_json Value） | **类型安全 IPC 是亮点** |
| 本地存储 | Drizzle ORM + SQLite | talksage-session（手写 SQL） | 手写 SQL 已够用 |
| 转写 | VideoDB 云（WS） | sherpa-onnx 本地流式 | 定位差异 |
| LLM | OpenAI SDK（经 VideoDB） | OpenAI 兼容（DeepSeek/Kimi/Ollama…） | 相同 |
| 额外 | MCP SDK、Visual Index（屏幕视觉索引）、Google Calendar | 无 | 可渐进借鉴 |

## 三、关键机制分析（对 talk-sage 的借鉴价值）

### 1. 会话指标引擎（conversation-metrics.service.ts）—— ⭐⭐⭐ 可直接移植
**纯统计、无 LLM**，从 final 段数组实时计算：
- **talk ratio**（我 vs 客户发言时长占比）、**pace**（WPM，取首尾段时间跨度为分母，clamp 50–250）、**questions asked**（`/\?/` 正则计数）
- **monologue 检测**（我连续发言 >45s）、**longest monologue**（段间 gap<2s 视为连续，算最长连续段）
- **interruption 计数**（不同说话人段重叠：`current.startTime < previous.endTime`）
- **平均段长**、词数、段数
- **会话健康分 0–100**（发言均衡 −偏差、独白 −15、语速过快/过慢 −分、提问 +分）与 **建议文案列表**（发言占比>65%、提问<2 等规则）
- **趋势对比**（距 50/50 的偏差变化 → improving/stable/declining）

**借鉴方向**：talk-sage 已有 SessionStats（speech_ratio/avg_rms/背景噪音）与质量评估（噪音/静音）。可在 pipeline 的 SessionStats 中**增量补三项**：我/客户 各自发言时长与占比（现有 total/speech 可按流分开）、语速 WPM、提问数（final 文本正则）、最长独白/打断数。纯本地、零成本、会中实时可展示。

### 2. 实时提示引擎（nudge-engine.service.ts）—— ⭐⭐⭐ 可直接移植
**规则驱动 + 限流**：
- 全局冷却 **2 分钟**（`cooldownMs`，避免打扰）；优先级顺序检查：talk_ratio → questions → pace → next_steps
- 触发条件带数据门槛：talk_ratio 需总发言 >60s；提问需通话 >3min 且 `提问数 < 期望×0.5`（期望=1 问/2 分钟）；next_steps 在 20min/30min 的 30s 窗口内触发一次
- 模板 + severity（low/medium/high）+ 可行动作（ask_question/pause/clarify/confirm），支持类型屏蔽与历史

**借鉴方向**：与 talk-sage 的"要点聚合/质量评估"互补。可在 pipeline 或前端按 SessionStats 增量跑同款规则，作为**会中右下角轻提示**（限流 2min）。纯规则无 LLM，性价比极高。

### 3. Live Assist（live-assist.service.ts）—— ⭐⭐ 中高
- 会中每 **20s** 轮询 LLM，喂最近滚动转写 + 可选会议上下文（名称/目的/准备问题/检查清单）+ 屏幕内容描述
- 输出 `say_this`（可说的话：回应/总结/待办/决策/承诺）与 `ask_this`（可问的问题：澄清/追问/深挖），各 0–3 条，要求可第一人称直接使用
- **去重**：previousSayThis/previousAskThis Set，避免重复建议
- System prompt 明确"只帮 User，不帮 Them"

**借鉴方向**：talk-sage 的 plugins（术语/翻译/简报）是 final 段触发、无 LLM 轮询式 assist。可新增一个 `LiveCoachPlugin`（AnalyzerPlugin 之外的新机制：按时间窗聚合 segments 调 LLM），或复用现有 PluginContext 的 LLM。建议**会后**先做（见 5），会中版作为可选开关（省 token）。

### 4. MCP 自动触发（mcp/*，intent-detector）—— ⭐⭐ 中
- **两级意图检测**：正则快路径（`/who is ... at .../` → crm_lookup、schedule/availability/doc_lookup/pricing…，带置信度 0.5–0.8）→ LLM 精确路径兜底
- Tool Aggregator（多 MCP server 工具去重聚合）+ Agent（自动调工具）+ Result Handler（结果内联展示 markdown/链接/结构化数据）+ 连接健康监控/鉴权
- 触发按"信息需求"自动，非用户显式请求

**借鉴方向**：talk-sage 的插件是固定管线；MCP 生态对接（工具即插即用）是**远期**方向。近期可借鉴其**意图正则库**思路：把"术语解释/简报检索"的触发从纯规则扩展到 LLM 判定（已有 term/translator 的骨架事件）。低优先级。

### 5. 三段式纪要生成（summary-generator.service.ts）—— ⭐⭐⭐ 高价值
**三个专精 prompt 并行（Promise.all）**，从同一 user prompt：
1. **Short Overview**：单段叙事（3–5 句、≤120 词、第三人称过去式、不逐字引用、不评论）
2. **Key Points**：按主题分组的要点 JSON，**每条归属发言人**（"Name did/said/raised/confirmed…"），2–5 主题 × 2–5 条
3. **Post-Meeting Checklist**：行动项/待办/遗留问题（负责人、期限），3–10 条，无则空数组

**借鉴方向**：talk-sage 的 notes 是单模板（standard_meeting/negotiation）+ 单次生成。可升级为**并行三段式**：在 notes crate 增加 `generate_trio`（overview + attributed key points + action items），复用现有 LLM 通道；这是"纪要质量"的直观提升点，且与现有模板可共存（模板保留、新增"智能纪要"按钮）。

### 6. 会议准备向导（meeting-setup.prompts.ts + service）—— ⭐⭐ 中
- 会议前：AI 生成 **3 个多选探测问题**（成功标准/风险/交付物，各 4 选项，要求非泛泛而谈）
- 用户作答 → AI 生成 **5–10 条会中检查清单**（live scorecard，要求可执行、不泛泛、按优先级排序）
- 会中检查清单逐条勾选；会后生成 post-meeting checklist 与之呼应

**借鉴方向**：talk-sage 可加"会议准备"（设置页或首页入口）：输入会议名称/描述 → 生成探测问题 → 生成会中清单 → 清单随会话展示。与知识库简报结合（简报命中客户背景后问题更准）。中优先级。

### 7. Workflow Webhook（workflow-webhook.service.ts + url-guard）—— ⭐⭐ 中
- 会议结束后把**结构化 payload**（callId/meeting 元数据/content: summary/topics/actionItems/checklist/transcript）POST 到 n8n/Zapier/CRM
- **安全细节**：webhook URL 存库后调用前**再次校验**（url-guard 防 SSRF：拒绝内网地址/伪造 host 重定向），失败拒绝调用

**借鉴方向**：talk-sage 有 headless server，加"会议结束 Webhook"（配置 URL 列表 + 结构化 JSON payload）成本低、与既有 `talksage serve` 天然契合；**url-guard 思路应一并借鉴**（本地应用调外部 URL 的 SSRF 防护）。中优先级。

### 8. 录音时长上限（recording-limit.service.ts）—— ⭐⭐ 小而精
- 上限 2h：**只记录制中时间**（暂停"记账"停表，分段累计），系统休眠（powerMonitor suspend/resume）不消耗额度
- 到期前 5 分钟 onWarning 预警；到期自动停止；计时器在 main 进程（隐藏窗口时 Chromium 会节流渲染进程 timer）

**借鉴方向**：talk-sage 的 `talksage record/listen --seconds` 已有基础；可加"默认 2h 上限 + 5min 预警 + 暂停记账"，逻辑照搬（Rust 侧无 powerMonitor 的话用系统休眠事件或退化为墙钟，至少"暂停不计时"可实现）。低-中优先级。

### 9. 书签（db/schema.ts bookmarks）—— ⭐ 低
- 会中标重要时刻：`timestamp + category`（important/follow_up/pricing/competitor/risk/decision/action_item）+ note

**借鉴方向**：talk-sage 历史详情页加"书签"（监听中快捷键标记 + 详情展示），SQLite 一表即可。低。

### 10. Markdown 结构化导出（markdown-export.service.ts）—— ⭐ 中
- 导出文件结构：`## Overview` → `## Key Discussion Points`（### 主题）→ `## Action Items` → 分段转写（### [起止时间]）→ `## Talk Ratio` → `## Speaking Pace`

**借鉴方向**：talk-sage 已有纪要生成（markdown）；补"完整导出"（转写 + 纪要 + 指标 + 质量徽章单文件 md）成本低，历史页一键下载。中。

### 11. 其他值得注意
- **会话恢复（session-recovery.service.ts）**：崩溃/重启后恢复进行中会话（本地优先应用的健壮性细节）——低
- **secure-store + encryption（utils/encryption.ts）**：API key 加密存储（Windows DPAPI/macOS Keychain 类比）——talk-sage 的 LLM key 明文在 toml，**可考虑加密**（低，依赖平台安全存储）
- **widget 小组件窗口**：始终置顶小窗显示 Say This/Ask This/Nudge——talk-sage 托盘已有，小组件窗是加分项（低）
- **Visual Index（屏幕视觉索引）**：屏幕内容 OCR/视觉描述喂给 live assist——依赖云端视觉模型，talk-sage 离线定位下不适用（不借鉴）

## 四、结论与建议优先级

| 建议 | 优先级 | 说明 |
|---|---|---|
| 会话指标增量（我/客户发言占比、WPM、提问数、独白、打断） | 高 | 纯统计无 LLM，直接进 SessionStats，会中可显示、会后可入 meta |
| 实时提示引擎（规则 + 2min 限流 + 模板） | 高 | 基于指标增量，纯规则，性价比最高 |
| 三段式纪要生成（overview + 归属发言人的要点 + 行动项） | 高 | 并行 3 prompt，直接升级 notes 质量 |
| Workflow Webhook（会议结束推送 + url-guard SSRF 防护） | 中 | 与 headless server 天然契合 |
| Markdown 结构化导出（转写+纪要+指标单文件） | 中 | 历史页一键下载 |
| 会议准备向导（探测问题 + 会中清单） | 中 | 与知识库简报结合更佳 |
| 录音时长上限（2h + 5min 预警 + 暂停记账） | 低-中 | 小而精，暂停记账可做 |
| MCP 自动触发 / 意图检测 | 低（远期） | 先做好本地插件，生态对接后置 |
| 书签 / 会话恢复 / API key 加密 / 小组件窗 | 低 | 锦上添花 |

**总评**：Call.md 与 talk-sage 定位互补（云端销售助手 vs 本地通用助理）。其**会中实时智能层**（纯统计指标 + 规则提示 + 会中 LLM assist）和**会后产出层**（三段式纪要、结构化导出、Webhook 自动化）是最值得借鉴的两块，且大多不依赖其云端栈，可平滑移植到 talk-sage 的本地 pipeline/plugins/notes 架构中。工程侧（tRPC 类型安全、shadcn UI、webhook SSRF 防护）作为长期参照。
