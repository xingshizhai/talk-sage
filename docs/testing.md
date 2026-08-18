# TalkSage v2 自动化测试

**全自动验证**：核心链路（音频 → VAD 分段 → 流式 ASR → 领域事件 → 前端行聚合）全部可由
wav 文件确定性驱动，无需人工交互，适合 CI。

## 1. 测试分层

| 层 | 工具 | 位置 | 内容 |
|---|---|---|---|
| Rust 单元测试 | cargo test | `crates/*/src` 内 `#[cfg(test)]` | 事件序列化、配置分层、引擎名解析、重采样/分块 |
| Rust 集成测试 | cargo test | `crates/*/tests/` | **真实模型**加载 + 文件输入全链路事件断言 |
| 前端单元测试 | Vitest | `web/src/**/*.test.ts` | 转写行聚合逻辑（partial/final/双说话人） |

集成测试**依赖仓库内模型**（`models/`，见下）；模型缺失时自动跳过（打印提示，不失败）。

## 2. 运行命令

```bash
# Rust 全量（单元 + 集成）
$env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\.tools\sherpa-onnx-archives"   # 构建期：sherpa-onnx 预编译库
cargo test --workspace

# 前端
cd web && npm install --ignore-scripts && npx vitest run

# 一键全量（PowerShell）
scripts/run_tests.ps1        # cargo test --workspace + vitest run
```

## 3. 测试覆盖明细

### Rust 单元测试
- `talksage-core`：DomainEvent JSON roundtrip、ResultStatus 区分
- `talksage-config`：默认值加载、用户文件覆盖、环境变量覆盖
- `talksage-asr`：EngineKind 解析（zipformer-en / paraformer-zh）
- `talksage-audio`：重采样（同率透传 / 48k→16k 长度 / 线性插值）、分块（单声道整块 / 立体声混 mono / 不足块滞留）

### Rust 集成测试（真实模型，模型缺失跳过）
- `talksage-asr/tests/asr_live.rs`：paraformer-zh 流式识别中文音频非空
- `talksage-pipeline/tests/pipeline_live.rs`：
  - 文件输入 → 状态事件链（AsrLoading→AsrReady→Recording→Idle）+ final 转写非空
  - partial 增量事件存在且先于 final
  - **双流**：user（中文文件）+ client（英文文件）→ 两个 speaker 都产出 final

### 前端测试（Vitest）
- `web/src/lib/transcript.test.ts`（6 用例）：
  - partial 原地更新不新增行
  - final 固化未完成行
  - 无 partial 的 final 直接新增行
  - 双说话人交替各自独立行
  - 每句 final 后重新 partial 新起一行
  - key 稳定复用

## 4. 依赖与模型准备（CI / 新机器）

| 项 | 来源 | 说明 |
|---|---|---|
| sherpa-onnx 预编译库 | GitHub Releases `sherpa-onnx-v1.13.5-win-x64-static-MT-Release-lib.tar.bz2` | 设 `SHERPA_ONNX_ARCHIVE_DIR` 指向含该文件的目录 |
| 模型 | `scripts/download_models.py all`（经代理下载） | `models/sherpa-onnx-streaming-paraformer-zh`、`...-zipformer-en-2023-06-26` |
| silero VAD | `scripts/download_models.py` 之外，`models/silero-vad/silero_vad.onnx` | 0.64MB |
| 测试音频 | 模型仓库自带 `test_wavs/0.wav`（已随模型下载） | 中文 10s / 英文 6.6s |

> 模型根目录探测顺序：`TALKSAGE_MODELS_DIR` → 相对 `CARGO_MANIFEST_DIR` 的 `../../models` → `models`。

## 5. 已知取舍

- 集成测试跳过策略：模型缺失时 `eprintln` 跳过（不失败），保证无模型环境 `cargo test` 仍绿。
- 双流集成测试约 20s（两个文件各自模拟实时节奏）。
- VAD 收尾：输入结束/停止监听时**强制 flush 当前语音段**（sherpa silero VAD 对纯静音收尾不触发段完成，已通过 `StreamWorker::shutdown` 处理并测试覆盖）。
