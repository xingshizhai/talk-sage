# BitNet CPU ASR 接入设计

**日期：** 2026-08-04  
**状态：** 已批准（方案 A：子进程包装 `asr_infer`）  
**关联：** [post-mvp 进度](../plans/2026-07-18-post-mvp-progress.md)

## 目标

将 [VibeVoice-ASR-BitNet](https://huggingface.co/microsoft/VibeVoice-ASR-BitNet) 经 [VibeASR.cpp](https://github.com/microsoft/VibeASR.cpp) 接入 TalkSage：

1. **实时**：英文侧可选 `transcribe.client.engine: bitnet`
2. **导入**：`transcribe.import.prefer_bitnet: true` 时优先 BitNet（整段推理），不可用则回退当前 client 引擎

## 架构

```
BitNetEngine(ASREngine)
  → 16 kHz float32 → 重采样 24 kHz → 临时 WAV
  → subprocess: asr_infer --vae-model … --lm-model … --audio … -t N --greedy
  → 解析 stdout 转写文本 → TranscriptSegment
```

中文侧仍为 FunASR；BitNet 仅替换 / 增强英文（client）路径。

## 配置

```yaml
transcribe:
  client:
    engine: faster-whisper | parakeet | bitnet
  bitnet:
    binary: ""       # asr_infer；空则 PATH / ~/.talksage/vibeasr/
    vae_model: ""
    lm_model: ""
    threads: 4
    timeout_seconds: 600
  import:
    prefer_bitnet: true
```

环境变量（可选）：`TALKSAGE_VIBEASR_ROOT` — 在其下查找 `asr_infer` 与 `*.gguf`。

## 非目标

- 不打包分发 `asr_infer` / 不自动下载 GGUF  
- 不做常驻 worker（二期）  
- 不替换中文 FunASR  

## 验收

- [x] factory 可选 bitnet  
- [x] 设置页可选 BitNet  
- [x] 导入 prefer_bitnet + 整段转写  
- [x] 缺二进制时错误信息可读  
- [x] 单元测试（mock subprocess）  
- [x] README / 设计文档更新  
