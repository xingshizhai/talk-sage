# TalkSage 语音自动化测试与模型评估使用手册

本文面向需要验证 TalkSage 音频链路、比较 ASR 模型或维护测试语料的开发与测试人员。架构和指标设计原理见 [evaluation-framework.md](evaluation-framework.md)。

## 1. 能做什么

评估工具包含两条相互独立的测试链路：

- ASR 固定语料评估：把同一批 wav 输入生产使用的 VAD、流式分块和 ASR 管道，比较错误率、实时性能与首段结果延迟。
- 真实麦克风检查：打开系统默认输入设备并录音，检查采样率、通道数、录音时长漂移、静音比例和削波比例。

普通 CI 运行固定语料评估，不要求物理麦克风。真实麦克风检查应在目标 Mac 或 Windows 机器上执行。

## 2. 首次准备

在项目根目录执行：

```bash
./scripts/talksage.sh env
./scripts/talksage.sh deps
./scripts/talksage.sh build
python3 scripts/evaluate.py prepare
```

各命令作用：

1. `env` 检查 Rust、Node、Xcode Command Line Tools、模型和 sherpa-onnx 库。
2. `deps` 下载基础模型和前端依赖。
3. `build` 生成前端资源并编译 Rust 工作区。
4. `prepare` 根据语料清单准备启动基准音频。

准备成功时会看到类似输出：

```text
copied sherpa-zh-mixed-001: evaluation/corpus/zh/sherpa-zh-mixed-001.wav
copied sherpa-en-reading-001: evaluation/corpus/en/sherpa-en-reading-001.wav
```

音频由本地 `models/` 复制产生并被 Git 忽略；参考文本和语料清单会纳入版本控制。

## 3. 运行自动化测试

运行完整单元测试和集成测试：

```bash
./scripts/talksage.sh test
```

它依次执行：

- Rust 工作区单元测试和集成测试；
- 前端 Vitest 测试；
- 评估编排器 Python 单元测试。

只验证评估编排器：

```bash
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
```

只验证音频设备无关逻辑：

```bash
cargo test -p talksage-audio
```

这部分覆盖重采样、立体声转单声道、固定分块、有界队列溢出、预处理、WAV 写入和异常录音恢复，不需要麦克风。

## 4. 比较 ASR 模型

评估全部已安装模型：

```bash
./scripts/talksage.sh evaluate
```

评估工具会：

1. 自动准备缺失的启动语料；
2. 检测已安装模型；
3. 使用同一语料逐个执行 `talksage bench`；
4. 计算综合分并应用质量门禁；
5. 输出推荐模型和 JSON 报告路径。

只比较指定模型：

```bash
python3 scripts/evaluate.py asr \
  --engines qwen3-asr
```

未安装的模型会标记为 `model_not_installed`，不会假装参与比较。当前产品可运行的本地段级引擎是 Qwen3-ASR：

```bash
python3 scripts/download_models.py qwen3-asr
```

Apple Silicon 的 Whisper large-v3-turbo Q5_0 可以通过 `whisper-metal` 目标预下载，但 Metal adapter 完成前不会进入可运行对比。Paraformer、Zipformer 和旧 sherpa Whisper 只用于历史回归，可通过 `legacy` 目标准备。下载后重新运行 `./scripts/talksage.sh build` 和评估命令。

## 5. 检查真实麦克风

录制并分析默认麦克风 10 秒：

```bash
./scripts/talksage.sh audio-test 10
```

运行时请以正常会议音量连续说话。录音位于：

```text
target/evaluation-capture/
```

检查指标：

| 指标 | 含义 | 常见异常 |
|---|---|---|
| `sample_rate` | 录音文件采样率 | 生产录音预期为 16000 Hz |
| `channels` | 通道数 | 生产录音预期为单声道 |
| `capture_drift_ratio` | 实际录音时长与请求时长的偏差 | 设备阻塞、回调中断 |
| `rms` / `peak` | 平均和峰值音量 | 权限被拒、输入源错误、增益太低 |
| `silence_ratio` | 近静音块比例 | 选错设备、没有说话、麦克风静音 |
| `clipping_ratio` | 接近满幅采样比例 | 麦克风增益过高、爆音 |

macOS 首次运行需要授予麦克风权限。如果没有弹出权限框，可在“系统设置 → 隐私与安全性 → 麦克风”中检查当前终端或 TalkSage App。

同时执行模型评估和硬件检查：

```bash
python3 scripts/evaluate.py all --hardware --seconds 10
```

不要在无人说话时把硬件检查加入无人值守 CI，否则静音门禁会按设计失败。

## 6. 查看报告

报告保存在：

```text
evaluation/reports/evaluation-YYYYMMDD-HHMMSS.json
evaluation/reports/latest.json
```

`latest.json` 始终指向最近一次结果。关键字段：

| 字段 | 说明 |
|---|---|
| `error_rate` | 中文 CER 或英文 WER；越低越好 |
| `rtf` | 当前为包含实时文件喂入的端到端实时系数；约 1 表示能跟随音频时钟 |
| `first_final_ms` | 从管道启动到第一个 final 段的时间，不是 partial 首字延迟 |
| `score` | 按配置权重计算的 0–100 综合分 |
| `gate_failures` | 没有通过的硬门禁 |
| `recommendation` | 通过全部门禁且综合分最高的模型 |
| `output` | 原始 bench 输出，用于定位具体识别结果 |

默认评分权重是准确率 60%、实时性能 25%、延迟 15%。参数位于 [evaluation.json](../evaluation/evaluation.json)：

```json
{
  "baseline_engine": "paraformer-zh",
  "quality_gates": {
    "max_error_rate": 0.35,
    "max_rtf": 1.35,
    "max_first_final_ms": 9000
  },
  "score_weights": {
    "accuracy": 0.60,
    "realtime": 0.25,
    "latency": 0.15
  }
}
```

实验候选模型不通过门禁不会导致整个命令失败；当前 `baseline_engine` 退化、所有候选均不可用或硬件检查失败才会返回失败。

## 7. 添加真实测试语料

推荐目录结构：

```text
evaluation/corpus/
├── manifest.json
├── zh/
│   ├── meeting-001.wav
│   └── meeting-001.txt
└── en/
    ├── meeting-002.wav
    └── meeting-002.txt
```

要求：

- WAV 使用 PCM16，推荐 16 kHz、单声道；
- `.txt` 是人工校对的完整参考文本；
- 音频与文本不能只靠同名存在，还必须加入 `manifest.json`；
- `id` 全局唯一并保持稳定；
- `language`、`scenario` 和 `source` 必须填写，便于后续按场景汇总；
- 不要把含客户隐私的音频提交到 Git。

清单示例：

```json
{
  "id": "zh-meeting-001",
  "audio": "zh/meeting-001.wav",
  "reference": "zh/meeting-001.txt",
  "language": "zh-mixed",
  "scenario": "far_field_keyboard_noise",
  "source": "internal-consented-corpus-v1"
}
```

正式模型选型建议准备至少 2–5 小时、500 条人工校对片段，覆盖近讲、远场、噪声、口音、中英混说、数字日期、产品名、短句、长句和停顿。开发集与冻结测试集应按完整会话划分，不能把同一会议相邻片段拆到两边。

## 8. CI 接入

基础 CI：

```bash
./scripts/talksage.sh build
./scripts/talksage.sh test
./scripts/talksage.sh evaluate
```

退出码：

- `0`：当前生产基线通过门禁，并且存在可推荐模型；
- `1`：基线退化、没有可用候选或硬件检查失败；
- `2`：模型、构建产物、语料或运行环境错误。

CI 应保存 `evaluation/reports/latest.json` 作为构建产物，以便比较不同提交的结果。模型评估需要真实模型文件，推荐使用带模型缓存的自托管 runner，避免每次下载大文件。

## 9. 常见问题

### 提示缺少 `target/debug/talksage`

执行：

```bash
./scripts/talksage.sh build
```

### 模型显示 `model_not_installed`

先执行 `./scripts/talksage.sh env` 查看缺失项，再用 `download_models.py` 下载对应模型。

### 麦克风检查没有生成 wav

检查麦克风权限、默认输入设备以及是否有其他程序独占设备。随后先运行：

```bash
./scripts/talksage.sh doctor
./scripts/talksage.sh audio-test 5
```

### 实时系数大于 1

当前指标包含按真实时间喂入音频的等待，因此略高于 1 并不等价于模型计算比实时更慢。超过默认 1.35 才表示整条管道明显落后。纯模型解码 RTF 将作为后续独立指标加入。

### 错误率超过 100%

WER/CER 在插入错误很多时可以超过 100%，通常表示模型语言不匹配、输出重复、参考文本不正确或端点切分异常。查看报告中该模型的 `output` 和逐文件结果定位。

### 启动语料已经通过，能否直接选定生产模型

不能。启动语料主要用于验证框架和发现明显回归。生产选型必须以 TalkSage 真实会议场景、人工校对且冻结的测试集为依据。
