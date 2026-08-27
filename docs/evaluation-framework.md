# TalkSage 自动化测试与语音评估框架

## 目标

这套框架回答两个不同的问题：

1. 音频设备与实时链路是否可靠：设备能否打开、采样格式是否正确、录音时长是否漂移、是否静音、削波或丢帧。
2. ASR 模型是否适合 TalkSage：在同一批真实会话语料上比较错误率、实时率和首字延迟，并用可配置权重给出候选模型，而不是根据单条演示音频选型。

框架复用生产代码路径：固定 wav 通过 `talksage bench` 进入与 App 相同的 VAD、流式分块、引擎池和 ASR 管道；真实麦克风通过 `talksage record` 进入 `AudioHub`。评估程序只负责准备语料、编排、质量门禁与机器可读报告。

## 测试金字塔

| 层级 | 是否需要硬件/模型 | 验证范围 | 运行频率 |
|---|---:|---|---|
| 音频单元测试 | 否 | mono 混音、48k→16k 重采样、固定分块、有界队列、WAV 原子写入/恢复 | 每次提交 |
| 确定性管道集成测试 | 需要基础模型 | wav→VAD→partial/final→事件→录音文件 | 每次提交或模型 CI |
| 固定语料模型评估 | 需要待测模型 | CER/WER、RTF、首字延迟、综合评分与回归门禁 | 模型/参数变更 |
| 真实设备冒烟 | 需要麦克风权限 | 默认设备打开、采样时长漂移、音量、静音、削波 | 发版前及目标机器 |
| 长稳测试 | 需要硬件 | 30–120 分钟采集的 overrun、内存和 CPU 趋势 | 发版候选版本 |

CI 不应依赖物理麦克风。设备无关逻辑由合成/固定 wav 确定性覆盖，真实设备测试作为 macOS/Windows 目标机上的独立门禁。

## 语料库

语料清单位于 `evaluation/corpus/manifest.json`。每条样本至少包含音频、人工参考文本、语言、场景和来源。`python3 scripts/evaluate.py prepare` 会从已经下载的 sherpa 示例中复制两条启动基准，避免把大体积音频重复提交到 Git。

当前两条样本只是框架自检，不足以做生产选型。正式中文会议集建议至少 2–5 小时、500 条人工校对片段，并按以下维度分层：

- 近讲/远场、安静/键盘声/空调声、单人/打断/重叠说话；
- 普通话、口音、中英混说、数字日期、产品名和行业术语；
- macOS 内置麦克风、耳机、USB 麦克风，以及 Windows 回环；
- 1–5 秒短句、15–30 秒长句和端点停顿。

数据集必须按“会话”划分开发集与冻结测试集，不能把同一会议的相邻切片分到两边。录音需取得授权；含客户信息的语料应仅存放在内部受控存储，不提交仓库。

## 指标与选型

- 中文主要看 CER，英文主要看 WER；规范化规则应固定，数字、大小写、全半角和标点策略不能随模型改变。
- 当前端到端实时系数 = 按实时节奏输入时的总耗时 / 音频时长，正常值约为 1；默认门禁 1.35 用于发现管道明显跟不上。它不是纯模型计算 RTF。
- 当前延迟项是首个 final 段延迟，不是首字 partial 延迟。离线段级模型没有真正 partial，不能与流式模型的首字体验混为一谈。
- 设备指标包括录音时长漂移、RMS/峰值、削波比例和静音块比例。
- 默认综合分为准确率 60%、实时率 25%、延迟 15%。只有通过全部硬门禁的模型才参与推荐。

生产选型不能只看全局平均值。最终报告应按场景输出 P50/P95，中文主场景设置更高权重。若未来的低延迟引擎和高准确率段级模型各有优势，可另行评估“快速预览 + 段尾修订”的双模型策略；当前生产链路只发布段级 final，不能把历史流式 partial 指标混入推荐分数。

## 使用

```bash
# 构建后准备语料，并评估所有已安装模型
./scripts/talksage.sh build
python3 scripts/evaluate.py prepare
./scripts/talksage.sh evaluate

# 只比较当前产品本地模型（CUDA: qwen3-asr；Apple Silicon: whisper-large-v3-turbo-metal）
python3 scripts/evaluate.py asr --engines qwen3-asr
python3 scripts/evaluate.py asr --engines whisper-large-v3-turbo-metal

# 真实麦克风测试（macOS 需终端或 .app 已获麦克风权限）
./scripts/talksage.sh audio-test 10

# 完整评估，并显式加入硬件层
python3 scripts/evaluate.py all --hardware --seconds 10
```

每次运行生成带时间戳的 `evaluation/reports/evaluation-*.json`，并刷新 `latest.json`。报告保留每个模型的原始 bench 输出，便于追查分词、空结果或某条语料失败。阈值、权重和候选模型在 `evaluation/evaluation.json` 配置。

退出码 `0` 表示当前生产基线通过门禁且至少有一个可用候选，`1` 表示基线质量退化、无可用候选或硬件检查失败，`2` 表示环境或执行错误。横评中的实验模型失败不会单独阻断 CI。生产基线由配置中的 `baseline_engine` 指定。

## 下一阶段

1. 录制并人工校对 TalkSage 自身会议语料，建立冻结的 `zh-meeting-v1`。
2. 将 bench 输出升级为逐样本 JSON，增加不按音频时钟等待的纯解码 RTF，并加入 partial 首字 P50/P95、模型加载耗时、峰值内存和端点延迟。
3. 在 macOS 与 Windows 自托管 runner 上执行 30 分钟长稳采集，读取 `AudioHub::overruns()` 并采集 CPU/RSS。
4. 为噪声、混响和不同增益生成可重复的增强版本，但始终保留未经增强的真实测试集作为最终依据。
