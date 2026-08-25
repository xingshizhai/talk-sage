# 模型管理架构

**最后更新：** 2026-08-25

## 定位

程序内「模型管理」是最终用户安装、续传、校验和删除模型的唯一推荐入口。`scripts/download_models.py` 只用于开发、CI、离线预置和旧基准模型准备。桌面端与 headless Server 都调用 `talksage-asr::models` 的同一套下载、校验和安装实现，传输层只负责注册取消标志和转发进度事件。

## 模型目录

解析顺序：

1. `TALKSAGE_MODELS_DIR`，不存在时自动创建；
2. 开发工作区或可执行文件旁已经存在的 `models/`；
3. `<TALKSAGE_DATA_DIR>/models/`，默认 `~/.talksage/models/`，自动创建。

正式安装包不会向 `.app` 或只读资源目录写模型。通过 `scripts/talksage.sh` 启动开发版时，脚本把 `TALKSAGE_MODELS_DIR` 指向仓库 `models/`，因此应用内下载和脚本下载不会各保存一份。

## 产品模型目录

| ID | 文件 | 状态 | 用途 |
|---|---|---|---|
| `qwen3-asr` | `sherpa-onnx-qwen3-asr-0.6b/` | 可选择 | CUDA/显式 CPU 本地段级识别 |
| `whisper-large-v3-turbo-metal` | `whisper.cpp-large-v3-turbo-q5_0/ggml-large-v3-turbo-q5_0.bin` | 可预下载 | Apple Silicon Metal adapter 的默认候选 |
| `punct` | `punct-ct-transformer/model.onnx` | 可选择的辅助模型 | 中英文标点恢复与语义分句 |

Paraformer、Zipformer、旧 sherpa ONNX Whisper 已从产品模型目录和产品下载 API 移除，只保留内部枚举解析与 `download_models.py legacy`，供自动化回归和历史对比使用。

`ModelProfile::selectable` 将“允许下载”和“已有可运行 adapter”分开。Metal adapter 完成前，large-v3-turbo 可以预下载，但不会出现在 ASR 引擎选择框或 OpenAI 兼容模型 API 中。

## 下载状态机

```text
未安装
  → 空间预检
  → downloading（同目录 .part，支持 HTTP Range）
  → extracting（归档模型写入 staging）
  → verifying（结构 / 最小大小 / 哈希）
  → 原子 rename
  → 已安装
```

- 网络中断保留 `.part`，下次从已有字节继续；用户主动取消会清理当前 `.part`。
- 下载源不接受 Range 时删除旧响应并从头写入，不会把两份内容拼接。
- Qwen3-ASR 同时存在压缩包与 staging，下载前按两倍归档大小加 256 MiB 余量预检。
- Metal 单文件按模型大小加 256 MiB 余量预检。
- 安装和删除在实时监听期间被禁止，避免加载中的模型目录发生变化。

## 完整性规则

- Qwen3-ASR：归档只解压到 staging；要求 `conv_frontend.onnx`、encoder、decoder 和 tokenizer 完整，encoder/decoder 不得小于 100 MiB，校验后原子替换正式目录。
- Whisper large-v3-turbo Q5_0：使用官方 SHA-1 `e050f7970618a659205450ad97eb95a18d69c9ee`。验证标记同时记录哈希和文件大小，未经验证或随后被截断的文件不算已安装。
- 标点模型：主源是 sherpa-onnx GitHub Release 的 `vocab272727-2024-04-12` 归档，只提取 `model.onnx`；公开 Hugging Face 文件是备用源。最终文件不得小于 200 MiB。

标点模型旧 `vocab500k` Hugging Face 地址不存在并返回 401，禁止再次使用。当前公开模型约 281 MiB，界面预计下载/安装大小显示为约 294 MB。

## 下载源

| 模型 | 主源 | 备用源 |
|---|---|---|
| Qwen3-ASR | sherpa-onnx GitHub Release | 暂无 |
| Whisper Metal | whisper.cpp Hugging Face 仓库 | 暂无 |
| 标点恢复 | sherpa-onnx GitHub Release | 正确的公开 Hugging Face `vocab272727` 仓库 |

代理统一读取 `HTTP_PROXY`、`HTTPS_PROXY` 和 `ALL_PROXY`。下载错误必须包含模型 ID、源地址和底层 HTTP 错误；存在主备源时同时报告两个错误。

## 日志与事件

底层下载器写入：目录、空间预算、源地址、续传 offset、每 10% 进度（总大小未知时每 64 MiB）、解压、校验、完成、取消和失败。桌面端与 Server 只补充任务提交/结束边界。日志保存在 `<data_dir>/logs/`。

前端通过 `DomainEvent::ModelProgress` 接收 `downloading`、`extracting`、`done`、`cancelled`、`error`。发现重启前遗留 `.part` 时显示“点击可继续”。日志进度经过节流，不能按 256 KiB 网络块逐条输出。

## 开发与测试

```bash
# 当前产品模型与公共模型
python3 scripts/download_models.py all

# 单独下载
python3 scripts/download_models.py qwen3-asr
python3 scripts/download_models.py whisper-metal

# 历史回归模型
python3 scripts/download_models.py legacy
```

下载器测试覆盖代理、取消清理、HTTP Range 续传、归档逐条读取、标点模型提取、SHA-1 标记、最小文件大小和空间预算。涉及 `127.0.0.1` 临时 HTTP 服务的测试在受限沙箱中需要网络绑定权限。
