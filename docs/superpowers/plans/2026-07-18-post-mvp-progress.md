# TalkSage Post-MVP 进度与后续计划

**日期：** 2026-07-18  
**关联设计：** [talksage-design.md](../specs/2026-06-18-talksage-design.md)  
**前置计划：** [phase1-mvp.md](./2026-06-18-talksage-phase1-mvp.md)（MVP 任务清单，历史归档）

---

## 目标

在 Phase 1 MVP 管道之上，完成合规与体验（P0）、架构可扩展性（P1）、产品化能力（P2），使 TalkSage 可日常试用；并为翻译/评估/谈判插件与真流式 ASR 留好接口。

---

## 已完成

### P0 — 体验与合规

| 项 | 实现要点 | 主要文件 |
|----|---------|---------|
| 术语去重 + 冷却 | 会话内已见缩写不再请求；`cooldown_seconds` | `plugins/term_explainer.py` |
| ASR warmup / 状态 | 启动后台预热；状态栏文案 | `core/pipeline.py`, `ui/main_window.py` |
| 录音同意 | 首次监听弹窗并持久化 | `ui/consent_dialog.py` |
| 屏享排除 | Windows `WDA_EXCLUDEFROMCAPTURE` | `ui/screen_share.py` |

### P1 — 架构增强

| 项 | 实现要点 | 主要文件 |
|----|---------|---------|
| 多后端 ASR | `transcribe.mode: local \| cloud` + factory | `core/asr/factory.py`, `openai_cloud_engine.py` |
| 渐进插件结果 | `PluginResult.result_id/status` + `analyze_stream` | `plugins/base.py`, `core/plugin_bus.py` |
| 会话落盘 | `~/.talksage/sessions/*.md` | `core/session_store.py` |
| 串音抑制 | Jaccard + 时间窗；ASR `run_in_executor` | `core/echo_filter.py`, `pipeline.py` |

### P2 — 产品化

| 项 | 实现要点 | 主要文件 |
|----|---------|---------|
| Setup Wizard | ASR / LLM / KB 首次引导 | `ui/setup_wizard.py` |
| ConversationState | 话题 / 问句 / 决策启发式 | `core/conversation_state.py` |
| 知识库 | 本地 md/txt + brief 插件 | `core/knowledge_base.py`, `plugins/brief_retriever.py` |
| 会后纪要 | LLM 生成并追加会话文件 | `core/notes_generator.py`, `main.py` |

**测试：** `pytest` 约 100+ 用例通过（以本地运行为准）。

---

## 下一阶段（Phase 3）建议顺序

### 3A — 会议辅助插件（优先）

1. **翻译插件** `translator`  
   - 触发：客户英文句 / 用户中文句  
   - UI：新增翻译区或复用简报区下方  
   - 使用低延迟模型（如 Groq）

2. **谈判 / 技术评估插件**  
   - 本地门控（问句、价格、交期关键词）+ 快慢模型  
   - 输出到建议区；复用 `ConversationState` + `KnowledgeBase`

### 3B — ASR 体验

3. **VAD / 流式切分** — 替代固定 3 秒块；评估 FunASR streaming  
4. **云端 WebSocket 流式 ASR** — 在现有 `ASREngine` 下新增后端，不破坏 factory

### 3C — 产品打磨

5. 知识库 embedding（可选，OpenAI-compatible `/v1/embeddings`）  
6. Claude 直连 Provider  
7. PyInstaller 打包；托盘图标；分区折叠  

---

## 验收清单（当前可用）

- [x] 本地双引擎转写 + 可选云端 Whisper API  
- [x] 术语解释不刷屏（去重/冷却/骨架）  
- [x] 双路采集 + 串音抑制  
- [x] 首次向导 + 录音同意 + Windows 屏享排除  
- [x] 会话自动保存 + 会后纪要  
- [x] 可选客户简报检索  
- [ ] 实时翻译区  
- [ ] 技术评估 / 谈判分析  
- [ ] 真流式 ASR  
- [ ] 安装包分发  

---

## 配置与数据路径（速查）

| 路径 | 用途 |
|------|------|
| `~/.talksage/config.yaml` | 用户配置 |
| `~/.talksage/sessions/` | 会话 Markdown |
| `config/defaults.yaml` | 内置默认 |
| `config/config.template.yaml` | 用户复制模板 |
