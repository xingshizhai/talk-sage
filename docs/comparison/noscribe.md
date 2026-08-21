# 对比分析：noScribe（kaixxx/noScribe）

> 调研日期：2026-08-20。来源：`D:\Work\github\noScribe`（GPL-3.0，v0.7.2，~86 文件，Python）。

## 一、项目定位

**纯本地的"访谈音频 → 带说话人标注的转写稿"流水线工具**：解决质性研究/记者访谈的逐字稿痛点（人工转写 1 小时音频需 5-10 小时），用本地 ASR（faster-whisper）+ 说话人分离（pyannote）自动产出高质量逐字稿，并配套编辑器做"听音校对"。

- 目标用户：质性研究者、记者、语言学家（需要逐字稿与数据保密的学术群体）
- 核心卖点：完全本地（"No cloud, no worries"）、说话人区分 + ~60 语言、内置校对编辑器（文本-音频同步回放）、三平台免费开源
- **重要澄清**：noScribe **没有 LLM 纪要/大纲生成、没有数据库**（文件即存储）；定位是"自动初稿 + 人工精校"（README 承认 1 小时音频最长转 3 小时、输出永远需人工校对）

## 二、技术栈

| 层面 | 技术 |
|---|---|
| 语言 | Python 3.10+，CI 矩阵 3.10/3.13 |
| GUI | **customtkinter**（非 PySide6）|
| ASR | **faster-whisper**（CTranslate2，CUDA/CPU 双通道）|
| 说话人分离 | **pyannote.audio 4.0**（本地模型：segmentation + embedding + PLDA，VBx 聚类）|
| 音频 | PyAV（ffmpeg 绑定）流式转 16kHz mono WAV，支持 seek/截断 |
| 中间表示 | AdvancedHTMLParser（DOM 级 HTML，非字符串拼接）|
| 导出 | HTML（默认，Word/QDA 兼容）、TXT、WebVTT（EXMARaLDA 兼容）|
| VAD | faster-whisper 内置 Silero VAD |
| 进程模型 | multiprocessing（spawn）子进程隔离重型推理 + 消息队列协议 |
| 分发 | PyInstaller 三平台 spec + NSIS；python-i18n（9 种 UI 语言）|

## 三、架构设计

```
音频文件（任意格式）
  → audio/convert.ToWav（PyAV 流式 → 16kHz mono WAV 临时文件，支持 start/stop 时间窗）
  → pyannote_mp_worker 子进程（说话人分离 → [start,end,label] 段列表）
  → whisper_mp_worker 子进程（faster-whisper 流式吐 segment，含 word_timestamps）
  → 主线程 on_segment() 每收到一段：
      a. adjust_for_pause()：用 VAD 静音区间修正段边界（±200ms + start>=end 回退保护）
      b. 暂停标记：(..) / "(X seconds pause)" / "(X minutes pause)"
      c. find_speaker()：与 diarization 段重叠率 ≥80% 取最短段 → S01/S02…；重叠说话 "//S02:" 前缀
      d. 按说话人变化新建 <p>；每 60s 或换人插入彩色时间戳 [hh:mm:ss]
      e. 每段包成 <a name="ts_start_end_speaker"> 锚点挂进 HTML DOM
      f. 每 5 秒 save_doc() 增量写盘（HTML/TXT/VTT 之一），崩溃可恢复部分稿
  → 最终 HTML（唯一真相源）→ html_to_text / html_to_webvtt 导出 TXT/VTT
  → 自动启动 noScribeEdit 校对；队列行 ✔(打开)/⟲(重跑)/X(取消)
```

关键组件：
- **TranscriptionJob / TranscriptionQueue**（main.py:299-555）：`JobStatus` 状态机（WAITING→AUDIO_CONVERSION→SPEAKER_IDENTIFICATION→TRANSCRIPTION→FINISHED/ERROR/CANCELED）+ 防输出冲突
- **管线编排 `_process_single_job`**（main.py:2238-2908）：ToWav → pyannote 分离 → whisper 流式转写，逐步更新状态
- **子进程隔离**：重库只在 spawn 子进程 import，父进程 `q.get(timeout=0.1)` 轮询；取消 `proc.terminate()`；统一消息协议 `{type: log/progress/segment/result}`
- **on_segment 增量构建**（main.py:2721-2857）：锚点命名同时是编辑器"点击文本→定位音频"与 VTT 导出的数据载体
- **CLI 与 GUI 双前端**：`HeadlessApp` + `--no-gui` 复用同一 job/queue 管线

## 四、优点

1. **流式转写 + 增量 DOM + 定时自动保存 = 崩溃安全**：每段即更新内存 DOM、每 5 秒落盘，出错/取消后队列行显示"✔ 打开部分稿"
2. **重型计算子进程隔离 + 结构化消息协议**：UI 永不冻结、可即时取消、子进程崩溃可检测并给可读错误
3. **分阶段加权进度 + 99% 封顶**：音频转换 5% / 分离 45% / 转写 50%（无分离时转写 95%），封顶 99% 防"假完成"
4. **说话人-段落对齐算法**（overlap_len + find_speaker）：重叠率 ≥80% 取最短段；重叠说话 `//` 包裹且不换段；VTT 导出转 `<v S01>` voice 标签——纯函数极易移植
5. **单一中间表示 + 按需转换**：只维护 HTML DOM 一份真相，TXT/VTT 由解析器按需转换，有撇号转义等回归测试
6. **健壮运行时容错**：坏包跳过计数、写盘失败自动改名另存、CUDA 报错检测 + 强制 CPU 重试、输出文件冲突检测
7. **模型热插拔 + 多语言 UI**：扫描 model.bin 注册模型、下拉动态刷新；9 种 UI 语言

## 五、不足

1. **3634 行单体 main.py**：GUI/管线/HTML 生成/子进程协议/CLI 全耦合；on_segment 是深埋闭包（nonlocal 一堆状态），无法单独测试（测试注释自认"TODO: replace with actual code after refactoring"）
2. **没有纪要/大纲/总结能力**：无 LLM 依赖，产品止步于转写稿 + 人工校对
3. **无数据库/会话管理**：转写散落为文件，无索引/检索；`config_dir/log` 明文存全部转写文本（README 自警告隐私风险）
4. **性能短板**：父进程为 VAD 解码整个文件 + 子进程再解一遍（音频解码两次）；顺序批处理无并行；每 5 秒全量序列化 DOM（O(全文)）；1 小时音频 2-3 小时
5. **代码质量债**：`version_higher()` 复制粘贴错误（两个循环 append 到同一列表）；大量注释死代码；python-i18n 停更 6 年；核心管线（worker/on_segment/队列状态机）**零测试覆盖**
6. **对实时/流式场景不友好**：两遍式（先 diarization 后转写）+ 数 GB 模型；说话人标签事后离线分配

## 六、对 talk-sage 的可借鉴点

### 直接可借鉴（高价值）

1. **流式增量持久化 + 部分结果恢复**：每收到一个 sherpa 段就写 SQLite（WAL 批量），会话中断/杀进程后历史详情页能恢复"转写进行到哪、已产出哪些段"——noScribe 用文件做、talk-sage 用 SQLite 可做得更干净
2. **说话人-段落对齐纯算法**（重叠率 ≥80% 取最短段；重叠 `//` 包裹且不换段）：与语言无关的纯函数，Rust 移植成本极低；talk-sage 若做多说话人或合并 diarization 结果到流式段可直接用
3. **VAD 静音区间修正段边界 + 暂停标记**（±200ms 吸附 + start>=end 保护；`(..)`/`"(X seconds pause)"`）：改善段边界与纪要时间戳可信度；暂停文案可入会议纪要
4. **分阶段加权进度 + 99% 封顶**：talk-sage 的实时指标（转写进度/流式 vs 离线阶段）避免"进度条 100% 却还在转写"
5. **worker 消息协议解耦**（{type: log/progress/segment/result} + 取消 terminate + 异常可感知）：Tauri 下对应"worker 线程 + 事件/Channel"，引擎崩溃恢复很重要
6. **统一中间表示 + 多格式导出器**：维护一份段列表（start/end/speaker/text），md/txt/vtt/srt writer 只做格式化；锚点命名（时间戳+说话人编码）支持"点击文本跳转音频"交互
7. **取消/重试/部分完成的 UI 状态机**（JobStatus 枚举 + 队列行按状态切 X/⟲/✔ + 区分 set_canceled 与 set_error）：历史详情页会话状态展示可直接借用
8. **参数化转写选项**：语言 auto 检测、口误开关用 hotwords 提示词软控制（prompt.yml vs prompt_nd.yml）而非硬过滤——talk-sage 的提示词/上下文注入可参考
9. **模型热插拔管理**：扫描模型目录、校验必要文件、允许放新模型即时生效（对应 sherpa-onnx 模型选择器）
10. **CLI 与 GUI 共享管线**：`HeadlessApp` + `--no-gui`——talk-sage 命令行批量转历史音频可参考

### 反向教训（不要照抄）

- **不要用文件当存储**：无数据库 + 日志明文含转写文本是隐私隐患；talk-sage 的 SQLite + 历史详情页更优，只需借鉴"增量保存"时机
- **不要 3600 行单体主类**：管线/GUI/导出拆开
- **音频不要解码两次**：talk-sage 流式场景应在采集/引擎层直接拿 PCM 做 VAD
- **警惕 whisper 幻觉**：静音/背景噪音会幻觉出"语法合理的假词"——纪要生成前应加"置信度/静音段剔除"步骤（talk-sage 的三段式纪要基于转写文本，若不过滤会掺入幻觉内容）

### 一句话总结

> 对 talk-sage 而言，noScribe 最大价值是**"长转写任务的工程健壮性"**：① 段级增量持久化与部分结果恢复 → SQLite；② 说话人-段落重叠匹配算法与 VAD 边界修正 → Rust 纯函数；③ worker 消息协议与取消机制 → Tauri 事件模型；④ 加权进度与 99% 封顶 → 实时指标。而**纪要/大纲生成、SQLite 会话、历史详情、实时双流**恰恰是 noScribe 没有、talk-sage 已经领先的部分。

### 关键文件索引

- 管线编排：`noScribe/main.py` — `transcription_worker`(2238)、`_process_single_job`(2345)、`on_segment`(2721)、`set_progress`(2133)、`_handle_cuda_fallback`(2953)
- 子进程：`noScribe/whisper_mp_worker.py`、`noScribe/pyannote_mp_worker.py`
- 音频转换：`noScribe/audio/convert.py`
- 导出：`noScribe/utils.py`（`html_to_text`/`html_to_webvtt`）
- 模型管理：`noScribe/transcription.py`
