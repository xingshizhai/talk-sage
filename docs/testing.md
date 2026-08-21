# TalkSage 自动化测试与评估

测试按“纯逻辑 → 边界组件 → 真实模型 → 前端 → 基准评估”分层。核心链路可由 WAV 文件确定性驱动；涉及模型、设备或网络的测试明确区分，避免把“跳过”误认为“已验证”。

## 1. 快速运行

macOS / Linux：

```bash
./scripts/talksage.sh test
```

Windows：

```powershell
.\scripts\run_tests.ps1
# 或
.\scripts\talksage.ps1 test
```

分开运行：

```bash
cargo test --workspace
cd web && npm test
```

macOS 首次构建、运行和依赖检查统一使用 `./scripts/talksage.sh doctor|build|run|test`。脚本会复用 workspace 根目录的 Cargo target，避免从不同目录重复产生一套 Rust 编译产物。

## 2. 测试分层

| 层 | 位置 | 重点 |
|---|---|---|
| 核心单元测试 | `crates/*/src` | 事件协议、采样时钟、配置合并、音频处理、端点状态、统计、说话人归属、数据库迁移 |
| 管道/插件测试 | `talksage-pipeline`、`talksage-plugins` | 有界队列、双流公平性、过滤器顺序、observer/finalizer 隔离、停止与 drain 语义 |
| 真实模型集成测试 | `crates/*/tests` | WAV → VAD → ASR → final event → recording / plugins / speaker attribution |
| 服务测试 | `talksage-server` | REST、WebSocket、OpenAI 兼容接口和会话 API |
| 前端测试 | `web/src/**/*.test.ts` | partial/final 聚合、智能断句、设置持久化、插件元数据、重点标记 |
| 评估基准 | `scripts/evaluate.py`、`talksage bench` | CER/WER、RTF、首字延迟、设备/场景元数据和模型横向比较 |

当前测试规模会随代码变化；不要在 CI 中依赖固定总数。2026-08-21 的基线为：pipeline 47、plugins 63、session 17、frontend 60。新增架构能力应至少有对应的确定性测试。

## 3. 新架构的关键不变量

- 音频回调只做有界 `try_send`；队列满可报告 overrun，但不得阻塞回调。
- 双流使用 round-robin 非阻塞轮询；一条空闲或繁忙的流不得饿死另一条。
- 文件输入按 deadline 节拍推进；暂停恢复后不得快速追赶暂停期间的积压。
- partial 只更新 hypothesis；只有 final 经过 `EventFilter` 并进入 committed、observer 与持久化。
- 被短段过滤或跨流去重吞掉的段，不得提交说话人状态或污染统计。
- 慢插件使用固定 worker 和有界队列；队列满、panic、取消、超时不得阻塞实时转写。
- `SessionWriter` 按 FIFO 串行写 SQLite；`finish` 返回前必须排空已提交事件，finalizer 随后才能读取会话。
- 新数据库保存结构化 `speaker_attribution`；旧数据库自动补列，旧行仍可读取。
- RMS 是实际均方根 `sqrt(mean(x²))`，统计必须使用真实样本数，不能把块平均再次平均。

## 4. 真实模型与测试音频

模型目录探测顺序：`TALKSAGE_MODELS_DIR` → workspace `models/`。使用：

```bash
python3 scripts/download_models.py all
talksage bench --dir corpus --engine paraformer-zh
talksage bench --dir corpus --engine zipformer-en
```

真实模型测试缺少模型时会打印跳过原因而不是失败。发布前必须在模型齐全的机器上执行一次，并检查输出中没有意外 skip。

基准音频建议保留以下维度及人工校对文本：

- 普通话：短句、停顿断句、句尾弱音、数字和银行卡等安全词。
- 专业术语：产品名、缩写、中英混说和用户词表命中。
- 英语：不同语速与口音。
- 声学条件：近讲、远讲、低音量、背景噪声、扬声器回声。
- 设备路径：麦克风、Windows 回环、WAV 文件；macOS 当前只自动验证麦克风/文件，系统音频回环仍是产品项。

音频与标注建议用同名文件组织，例如 `sample.wav` + `sample.txt`，并记录设备、采样率、语言、场景和预期角色。真实会议录音进入仓库前必须脱敏并确认授权。

## 5. 评价指标

模型比较至少记录：

| 指标 | 用途 |
|---|---|
| CER / WER | 中文字符、英文单词准确率 |
| RTF | 推理耗时 / 音频时长；实时场景必须稳定小于 1 |
| 首字延迟 | 用户感知的响应速度 |
| final 延迟 | 停顿后提交完整句子的速度 |
| 句尾召回 | 检查停止/端点 flush 是否丢最后一个字 |
| 断句 F1 | 检查停顿和标点边界 |
| 术语准确率 | 用户词表和专业词识别能力 |
| speaker role accuracy | `owner/client/unknown` 结构化归属准确率 |
| overrun / dropped plugin jobs | 设备或慢插件压力下的稳定性 |

模型选择不要只看 CER：本应用优先满足实时性，再在可接受的 RTF 与内存范围内比较准确率、断句和术语表现。

## 6. CI 与本机差异

- 纯单元测试不依赖麦克风、GPU、模型或公网，应始终执行。
- 真实模型测试可在普通 CI 跳过，但发布门禁必须有模型环境的独立任务。
- 麦克风权限、默认设备、回环设备和 GUI 需要目标操作系统上的 smoke test，不能由 WAV 集成测试替代。
- 某些服务测试需要绑定本地端口；受限 sandbox 禁止 bind 时，应在正常本机/CI 环境复验，而不是删除该测试。
- LLM 与 Webhook 测试使用 mock；真实外部服务只做可选 smoke test，避免测试产生费用或发送用户数据。

## 7. 回归检查单

改动音频/VAD/ASR：运行 audio、asr、pipeline 和固定语料 benchmark；改动插件/持久化：运行 plugins、pipeline、session；改动事件字段：同时运行 core、server、Tauri 类型生成/编译和 frontend；改动停止流程：验证最后一个字、WAV 可播放、writer drain、finalizer 可见完整数据。
