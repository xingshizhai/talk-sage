# 参考项目研究：Whisper Flow（dimastatz/whisper-flow）

> 调研日期：2026-08-20。来源：`D:\Work\github\whisper-flow`（MIT，Python，53 文件，核心源码 7 模块 + 测试 10 + 文档 9）。

## 一、项目定位

**把批处理 Whisper 改造成实时流式转写服务**：客户端把音频切成小块（chunk）实时推送，服务端立即回推**部分结果（partial）**和**最终结果（final）**，延迟目标亚秒级（实测 M1 均值 ~275ms）。

- 目标用户：需要实时 STT 嵌入产品的开发者（pip 库形态）、自托管/air-gapped + 远程 GPU 场景（VS Code 语音输入、会议转写、字幕）
- 核心卖点：实时性（<500ms）、简单协议（WebSocket 二进制 PCM 进、JSON 事件出）、库/服务双形态、**零配置流式分段**（客户端无需 VAD）
- 注意："流式"实为**窗口重转写**（re-transcription of a growing window），非 Whisper 增量解码；README 承认"速度提升可能牺牲准确率"

## 二、技术栈

| 维度 | 内容 |
|---|---|
| 语言 | Python 3.8+（开发 3.12） |
| ASR | openai-whisper（20250625），CPU/GPU 自动选择 |
| 服务 | FastAPI + Uvicorn，WebSocket `/ws` |
| 音频 | PyAudio，16kHz/mono/int16 PCM |
| 测试 | pytest + jiwer（WER）、black、pylint、覆盖率门禁 |
| 形态 | FastAPI 服务 + pip 库；流式 + 批处理（/transcribe_pcm_chunk）双模式 |

## 三、架构设计

```
客户端 PyAudio 采集 16kHz PCM
  → fast_server.websocket_endpoint（x-api-key 校验 / 会话上限 128 / 注册会话）
  → TranscribeSession（Queue 解耦生产者/消费者，asyncio 后台循环）
  → 主循环（每 10ms）：window.extend(get_all(queue))
      → trim_window(1000 块 ≈64s 上限)
      → safe_transcribe（asyncio.wait_for 30s 超时 → None 静默跳过）
      → model.transcribe(整窗) → {"is_partial": true, "data": {...}, "time": 耗时}
      → should_close_segment？（文本连续相同 cycles>=1 → 清窗，is_partial=false）
  → websocket.send_json(result)（partial 与 final 都推）
```

关键组件：
- **分段机制** `should_close_segment()`（streaming.py）：不依赖 VAD，用"**文本连续 N 轮不变化 → 认为句子说完**"启发式；同时解决"何时出 final"与"窗口何时截断"
- **回调注入** `TranscribeSession(transcribe_async, send_back_async)`：传输与逻辑解耦，测试用假回调全链路测通
- **模型单例**：全局缓存 + 双检锁 + lifespan 启动预加载（首连零等待）
- **护栏**：推理 30s 超时、窗口 1000 块上限、API key、会话上限、上传 25MB 上限、模型路径穿越防护

## 四、优点

1. **三层解耦、可测性极强**：`TranscribeSession` 依赖注入两个回调，测试无需 GPU/WS/音频硬件即可测通流式链路；jiwer 对 LibriSpeech 断言 WER<0.1
2. **无 VAD 的文本稳定性分段**：`cycles + 文本相同` 状态机仅 20 行，同时解决 final 时机与窗口截断
3. **健壮性护栏齐全**：超时/窗口上限/空窗跳过/鉴权/会话上限/路径穿越防护，`test_hardening.py` 逐项回归
4. **质量门禁进 CI**：3.11/3.12 矩阵跑 black + pylint（fail-under 9.9）+ pytest（cov ≥95%）；benchmark 测试真实起服务推 LibriSpeech 断言延迟统计与 WER
5. **配置环境变量化 + 安全默认值**：`WF_*` 前缀 10 个参数全可调，源码零魔法数字
6. **模型单例 + 启动预加载 + 优雅停机**：双检锁防并发双加载；shutdown 排空会话；WS finally 必清理注册表

## 五、不足

1. **整窗重转写，非真正增量流式**：每轮把窗口内全部音频重跑 Whisper，随说话变长延迟劣化、CPU 高；partial 文本会回退重写（前端需接受抖动）
2. **分段与真实停顿脱节**：只依赖文本稳定，与说话人停顿无关——长句无停顿可能提前截断；whisper 对同一窗口每次吐不同文本时 cycles 永不达标
3. **并发与背压缺失**：单模型全局串行共享、无界 Queue（发送快于推理时内存无上限）、WS 无限流；延迟指标测的是推理耗时而非端到端
4. **事件协议简陋**：无协议版本/序号/时间戳/流结束信号/ack；`safe_transcribe` 失败**静默丢弃**，客户端感知不到出错
5. **chat_room 实验代码未完成**：顶层 import pytest、assert 当控制流、async 未 await
6. **文档与代码脱节**：已实现的 P0/P1 项仍列在 backlog；72MB 模型直接进 git；Docker 端口文档不一致

## 六、对 talk-sage 的可借鉴点

1. **Partial/Final 双态事件协议（最值得借鉴）**：`{"is_partial", "data"}` 把即时反馈与可提交结果分层——talk-sage 流式引擎出 partial（实时渲染）、VAD 切段后离线引擎出 final（提交 SQLite）；可加"流式引擎连出 N 条相同文本时提前提升为 final"作 VAD 漏切保险（对应 whisper-flow 的 `cycles`）
2. **回调注入的会话抽象 → Rust trait 化**：定义 `trait SegmentSink { on_partial/on_final }` + `trait Transcriber`，双流各建会话实例各注册 sink——可单测（mock sink 断言事件序列），流式/离线引擎做成可切换实现
3. **有界窗口 + 推理超时护栏**：离线引擎段级音频累积设上限（超限强制切段）、推理挂超时；比 whisper-flow 更进一步——**把丢弃事件推给 UI**（其静默丢弃是反面教材）
4. **模型启动预加载 + 单例复用**：Tauri setup 阶段异步加载模型（带进度/失败状态）；`/ready` 报告 model_loaded 的思路可移植为"模型就绪状态"实时指标
5. **会话注册表生命周期管理**：窗口关闭/退出时统一 stop 所有任务、未提交 final 先落 SQLite 再销毁
6. **基准测试进 CI（WER + 延迟双指标）**：黄金音频集 + 期望文本，CI 对每引擎断言 WER 上界与延迟分位数——**这是 talk-sage 目前最缺的一环**（有 `talksage bench` 可对接）
7. **反面借鉴**：不整窗重转写；双流用有界 channel + 背压；不静默吞错；双流并行时确认引擎线程安全模型；延迟指标测端到端

## 七、总结

whisper-flow 是把批处理 Whisper 改造成实时流式服务的教科书级小项目（~400 行核心代码）：窗口化重转写 + 文本稳定性分段 + 双态事件协议 + 完整质量门禁。工程纪律（三层解耦、回调注入、95% 覆盖率、CI 双矩阵、基准测试）比多数同规模开源项目强；短板是"伪流式"推理、无背压并发、简陋差错协议。对 talk-sage 最有价值的是**事件协议设计、会话抽象与生命周期管理、分段稳定化启发式、质量门禁+基准进 CI 的工程方法论**——与 talk-sage 的 Rust/Tauri/sherpa-onnx 架构可直接对应移植。
