# 参考项目研究：Dictata（AntoineChatry/Dictata）

> 调研日期：2026-08-26。来源：GitHub 仓库（`https://github.com/AntoineChatry/Dictata`，Rust，本地全离线听写）。

## 一、项目定位

**100% 本地、系统级全局语音听写（voice dictation）桌面应用，仅支持 Windows**：按全局热键开始说话、再按一次结束，语音在本地转写为文本并**自动粘贴进当前活动应用**。主打"数据不出机器"，转写由内置多后端引擎完成。

- 定位对比：Dictata = **个人听写工具**（热键 → 说话 → 文本进光标处，替代手动打字，面向单人多场景）；
  talk-sage = **会议转写助手**（双通道实时转写 + 说话人区分 + 会中智能 + 会后纪要）。两者都做本地 ASR，但**交互范式完全不同**：Dictata 是"输入法式"的全局热键听写，talk-sage 是"会议室录音机 + 分析台"。
- 独特亮点：**无需打开应用窗口**，热键即用、自动粘贴、浮动波形 dock；流式模式下边说边插入文本（逐句定稿，无回溯）。

## 二、技术栈对比

| 维度 | Dictata | talk-sage | 备注 |
|---|---|---|---|
| 桌面壳 | 原生 Win32 + 系统托盘（无 WebView） | Tauri 2（WebView） | Dictata UI 是 egui 即时模式 |
| UI 框架 | eframe/egui 0.34（原生 Rust GUI） | React/Vite/TS（手写样式） | egui 轻量但生态弱于 Web |
| 音频 | cpal（mic / WASAPI loopback / 混合） | cpal + WASAPI loopback（双流独立） | 相同底层 |
| ASR 引擎 | whisper-rs（whisper.cpp，Vulkan GPU）+ 可选 ONNX 后端（Parakeet/SenseVoice/Moonshine/Zipformer） | sherpa-onnx（流式 Paraformer/Zipformer/Whisper/Qwen3） | **多后端插件化思路可借鉴** |
| 全局热键 | global-hotkey 0.8 | 无（窗口内操作） | ⭐ 听写场景刚需 |
| LLM 后处理 | 本地 OpenAI 兼容（LM Studio/Ollama），默认关 | OpenAI 兼容（DeepSeek/Kimi/Ollama…） | 相同，Dictata 强调"本地 LLM" |
| 配置 | `config.json`（exe 旁 / DICTATA_HOME） | `talksage.toml`（数据目录） | Dictata 用 JSON，talk-sage 用 TOML |
| 历史 | `history.jsonl`（JSON Lines 追加） | SQLite（sessions/segments/terms/key_points） | talk-sage 结构化更强 |
| 许可证 | MIT + **Commons Clause**（禁商用销售） | MIT | 注意：Dictata 禁 SaaS/售卖 |

## 三、关键机制分析（对 talk-sage 的借鉴价值）

### 1. 全局热键 + 自动粘贴（⭐ 可直接借鉴，但属听写范式）
- `global-hotkey` 注册系统级热键（默认 `Ctrl+Alt+Space`），按一下开始、再按结束；长按 `Esc`（约 0.5s）取消本次。
- 完成后经 `enigo` 模拟按键把文本**粘贴进当前焦点应用**（存剪贴板 → 模拟 Ctrl+V），并自动还原剪贴板。
- 浮动 dock（波形 + 状态）不抢焦点，可调大小/透明度/位置。

**对 talk-sage 的启发**：talk-sage 当前是"打开应用 → 点监听"模式。若未来做**快速听写入口**（如托盘图标热键触发单人听写并粘贴），这套机制可直接移植；`global-hotkey` + `enigo` + `arboard` 都是成熟 crate。但会议转写场景不依赖此范式，优先级低。

### 2. 多后端引擎抽象（⭐⭐⭐ 架构借鉴价值最高）
`src/engine/` 定义了 `AsrEngine` trait，按**模型形态自动选择后端**：
- ggml `.bin` 文件 → whisper.cpp；
- ONNX bundle 目录 → 按其结构匹配 Parakeet / SenseVoice / Moonshine / Zipformer。
- 各后端用 **Cargo feature 隔离**（默认只带 whisper；`parakeet`/`sensevoice`/`moonshine`/`zipformer` 各自编译期开关，重型原生依赖互不牵连）。
- `engine/pipeline.rs` 提供 **VAD 门控转写管道**：Silero VAD 切出语音区段 → 逐段转写 → 丢弃静音 → **把片段拼回原始时间轴**（对两类后端统一处理：whisper/Parakeet 自带时间戳段；Zipformer/SenseVoice/Moonshine 无时间戳则按语音区段合成）。

**对 talk-sage 的启发**：
- talk-sage 的 `EngineKind` 已是多后端枚举（Paraformer/Zipformer/Whisper/Qwen3/Aliyun），但**模型目录结构是"一个引擎一个目录约定"**，没有 Dictata 那种"从文件形态自动探测后端"的灵活性。若未来支持任意 ONNX 模型导入，可借鉴"形态 → 后端"探测。
- **Cargo feature 隔离重型依赖**（ort/onnxruntime 等）是很好的工程实践：talk-sage 的 sherpa-onnx/whisper.cpp 目前是全局依赖，编译时间长；按需 feature 可缩短开发期编译。

### 3. 流式听写：暂停切块 + 逐句定稿（⭐⭐ 思路参考）
`src/streaming.rs`（移植自 freewhisper/streaming.py）：
- 按 RMS 静音（0.7s 尾静音）切块，每块立即转写并**一次性定稿插入**（无回溯、文本绝不重写）；
- 已发射文本的尾部 200 字符重新注入 prompt（`PROMPT_TAIL`），维持上下文；
- 15s 强制切块（`MAX_CHUNK_S`），防单块过长；
- 静音阈值 `SILENCE_RMS=0.008`、最小有声 0.6s（防幻觉）。

**对 talk-sage 的启发**：talk-sage 的流式是"VAD 段内逐块增量出 partial、段尾定稿"，且已有 endpoint（停顿稳定/强制静音）与标点恢复做语义分句——比 Dictata 的"静音切块"更精细。但 Dictata 的 **`PROMPT_TAIL` 尾部上下文注入**值得借鉴：talk-sage 流式引擎目前依赖引擎内部上下文（sherpa-onnx 自带），若用离线段模型做流式（Qwen3/Whisper 切块），尾部 prompt 注入可显著提升连贯性。

### 4. 模型加载哨兵文件（⭐⭐ 可靠性细节，直接可借鉴）
`model_sentinel_path()`：加载模型前写 `model_loading.tmp` 标记，成功加载后删除；若加载中途 `abort()`（whisper.cpp 对损坏 ggml 文件会直接 abort，无法 catch），下次启动读哨兵 → 自动回退默认模型，**打破"坏模型导致每次启动崩溃且无出路"的死循环**。

**对 talk-sage 的启发**：talk-sage 此前修过 **Qwen3 0 字节模型导致 native 崩溃**的问题（is_available 校验非空 + 加载前大小校验）。Dictata 的哨兵方案是**启动期自愈**的补充：即使某个模型在加载中 crash，下次启动也能自动恢复默认，而不是反复崩溃。可在模型加载路径加"加载中标记 + 启动恢复"。

### 5. LLM 输出模式与提示词工程（⭐⭐ 借鉴）
- 输出模式（`modes.rs`）：raw → 词典替换（大小写不敏感，逐词替换）→ 可选 LLM 重排（邮件/消息/列表…），自定义 prompt。
- **语言指令的提示词陷阱**（实测 qwen3-4b）：输出语言指令若放在模式 prompt 之后会被模型当作最后一条强指令而"忠实复刻原文"；放在**模式 prompt 之前**的独立一行才有效。这是很有价值的实测经验。
- LLM 可用性探测：`GET /models` 轻量检查（对应 talk-sage 刚加的"检查"按钮，但 Dictata 是启动时探测本地端点）。

**对 talk-sage 的启发**：
- talk-sage 已有词典替换（terminology）与 LLM 纪要，但**没有"输出模式"（把一段转写重排成邮件/列表）**。可作为轻量插件（本地 LLM 或 DeepSeek）扩展。
- **LLM 语言指令放置顺序**的经验可直接复用到 talk-sage 的纪要/要点 prompt（确保输出语言与内容要求不冲突）。

### 6. 配置健壮性（⭐ 工程实践）
- 手写 JSON 配置 + `#[serde(default = ...)]` 逐字段默认，缺键自动补齐以兼容升级；
- `DICTATA_HOME` 环境变量覆盖配置目录（对应 talk-sage 的 `TALKSAGE_DATA_DIR`）；
- 发布包**不携带 config.json**（首跑自写默认），避免泄露开发机个人数据。

## 四、功能矩阵对比

| 功能 | Dictata | talk-sage | 备注 |
|---|---|---|---|
| 本地离线转写 | ✅（whisper/ONNX 多后端） | ✅（sherpa-onnx 流式/离线） | 核心相同 |
| 全局热键听写 + 自动粘贴 | ✅（核心卖点） | ❌ | 听写范式，talk-sage 未做 |
| 流式边说边出 | ✅（静音切块逐句定稿） | ✅（VAD 段 + partial） | 实现不同，talk-sage 更精细 |
| 双通道（麦 + 系统） | ✅（混合进一路） | ✅（双流独立 + 说话人区分） | talk-sage 更强 |
| 说话人区分 | ❌ | ✅（wespeaker 声纹/在线聚类） | — |
| 录音保存/回放 | ❌（无录音留存） | ✅（分轨 + master 双声道） | talk-sage 独有 |
| 会话历史 | ✅（jsonl 文本） | ✅（SQLite 结构化） | — |
| 会中指标/要点聚合 | ❌ | ✅（指标 + 规则/LLM 要点） | — |
| 会后纪要 | ❌ | ✅（模板纪要/三段式/要点整理） | — |
| 文件转写 | ✅（ffmpeg 解码） | ✅（导入 + 离线转写） | 相同 |
| 模型管理 | ✅（内置库 + HF 搜索 + 推荐） | ✅（下载/取消/校验） | Dictata 有 HF 搜索可借鉴 |
| 多语言 UI | ✅（fr/en/es） | ✅（中文界面） | — |
| GPU 加速 | ✅（Vulkan，AMD/Intel/NVIDIA） | ⚠️（探测 + Apple Metal 路线） | Dictata Vulkan 覆盖面广 |
| 系统托盘 | ✅ | ✅ | 相同 |

## 五、结论与建议

**Dictata 与 talk-sage 面向不同场景**（个人听写输入 vs 会议转写分析），架构上 talk-sage 在"会话分析能力"上全面领先；Dictata 值得借鉴的有三点：

1. **模型加载哨兵自愈**（⭐⭐⭐ 建议采纳）：加载中标记 + 启动回退默认模型，杜绝"坏模型反复崩溃"。直接对照 talk-sage 此前 Qwen3 0 字节崩溃修复做补充加固。
2. **多后端引擎的 Cargo feature 隔离**（⭐⭐ 中期工程优化）：把 sherpa-onnx/whisper.cpp 等重型依赖按 feature 隔离，缩短开发期编译、降低包体。talk-sage 的 `EngineKind` 已是枚举抽象，落地成本低。
3. **LLM 提示词实测经验**（⭐⭐ 立即可用）：输出语言指令放模式 prompt 之前；`GET /models` 探测端点对应已有的"检查"按钮，思路一致。

**不建议照搬**：全局热键 + 自动粘贴（听写范式，与会议场景冲突）；egui UI（talk-sage 已用 React/Tauri，生态更好）；Commons Clause 许可（若未来考虑直接引代码，需注意其禁商用条款——只借鉴思路、不复制代码最稳妥）。
