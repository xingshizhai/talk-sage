# 专业术语增强使用说明

在“设置 → 术语纠错”中启用功能。术语和纠错均在下一次监听时生效。

## 术语表

每行填写一个本次会议可能出现的人名、产品名、缩写或行业术语：

```text
TalkSage
Paraformer
WhisperLiveKit
RAG
向量数据库
```

Qwen3-ASR 会使用模型原生热词字段。Zipformer 使用 `modified_beam_search` 上下文偏置；下载器会从 `bpe.model` 自动生成 sherpa 所需的 `bpe.vocab`，再由 sherpa 编译普通文本术语。缺少该词表时会安全回退到原来的 `greedy_search`。Paraformer 当前不支持这条 transducer 热词路径，仍使用纠错表。

## 纠错表

对于实时模型已知且稳定的误识别，每行使用 `误识别 => 标准术语`：

```text
拓思者 => TalkSage
怕热佛母 => Paraformer
外斯珀莱夫克特 => WhisperLiveKit
```

纠错同时作用于 partial 和 final，采用长规则优先的确定性替换，不调用 LLM、不阻塞音频线程。不要配置过短或含义宽泛的错误词，例如 `模型 => Paraformer`，否则容易误替换正常语句。

## 评估效果

在 `evaluation/corpus/manifest.json` 的样本中增加 `terms`：

```json
{
  "id": "zh-meeting-001",
  "audio": "zh/meeting-001.wav",
  "reference": "zh/meeting-001.txt",
  "language": "zh-mixed",
  "scenario": "technical_meeting",
  "source": "internal-consented-corpus-v1",
  "terms": ["TalkSage", "Paraformer", "向量数据库"]
}
```

运行：

```bash
./scripts/talksage.sh evaluate
```

报告会增加：

- `term_recall`：参考文本中的目标术语被正确识别的比例；
- `term_precision`：识别出来的受评术语中正确出现的比例；
- `term_expected`：本次参与统计的术语数量。

启动语料只用于验证功能。专业术语优化应使用真实会议录音，并同时加入“不应出现该术语”的发音相似反例，防止热词或纠错规则过强。
