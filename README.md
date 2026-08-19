# TalkSage v2

实时个人 AI 会议助理——在视频会议或面对面洽谈中，实时识别对方讲话，提炼关键内容、解释术语、关联知识，帮助你及时回应。

**v2 为推翻重设计**：Rust 全栈核心 + Tauri 2 桌面壳 + React Web UI（架构参考 DeepSeek Harness 的工程形态）。旧版 Python/PySide6 实现已移除。

## 功能

- **双流实时转写**：user（麦克风，中文 streaming paraformer）+ client（回环/文件，英文 streaming zipformer），各自 VAD 分段 + 流式增量出字
- **全本地处理**：sherpa-onnx 流式 ASR（CPU 推理，RTF ≈ 0.04，比实时快 20+ 倍）
- **会议辅助插件**：术语解释（英文缩写 → LLM 中文解释，冷却+去重+先骨架）、客户简报检索（本地知识库 Jaccard）、实时中英互译——独立线程执行不阻塞音频链路
- **会话持久化**：监听自动落库 SQLite（`TALKSAGE_DATA_DIR`/`~/.talksage/sessions.db`），历史页可浏览/搜索/查看详情
- **纪要模板化**：基于会话转写+术语按模板（标准会议/商务谈判/技术评审/每日站会）LLM 生成 Markdown 纪要，保存到会话
- **导入转写**：`talksage import <wav>` 离线转写并保存为新会话
- **Headless 服务（多设备）**：`talksage serve` 启动本地服务，**手机/平板浏览器**即可访问全部功能（同局域网需 `--host 0.0.0.0` + token）
- **领域事件驱动**：转写/术语/翻译/简报/状态事件经 IPC（Tauri）或 WS（headless 预留）推送，前端分区实时渲染
- **全自动测试**：核心链路（wav → VAD → ASR → 事件 → 插件 → 前端行聚合）确定性可测，`scripts/run_tests.ps1` 一键全量

## 技术栈

| 层 | 选型 |
|---|---|
| 核心 | Rust workspace（crates: core / config / asr / audio / pipeline / cli） |
| 应用壳 | Tauri 2（Windows WebView2 / macOS WKWebView） |
| 前端 | Vite + React + TypeScript |
| ASR | [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) 1.13（streaming paraformer-zh + zipformer-en，静态链接预编译库） |
| VAD | sherpa-onnx silero VAD（端点分段） |
| 音频采集 | cpal（麦克风）；WASAPI loopback（系统回环，Windows） |

## 仓库结构

```
├── crates/
│   ├── talksage-core/        # 领域模型与事件（DomainEvent，传输无关）
│   ├── talksage-config/      # 分层配置（默认 + talksage.toml + 环境变量）
│   ├── talksage-asr/         # sherpa-onnx 流式引擎封装（双引擎）
│   ├── talksage-audio/       # cpal 麦克风采集 + WASAPI 回环 + 重采样
│   ├── talksage-pipeline/    # 双流管道：VAD 分段 → 流式 ASR → 插件 → 领域事件
│   ├── talksage-llm/         # OpenAI 兼容 LLM Provider
│   ├── talksage-knowledge/   # 本地知识库 Jaccard 检索
│   ├── talksage-plugins/     # 术语解释 / 简报检索 / 翻译
│   ├── talksage-session/     # SQLite 会话存储（可检索历史）
│   ├── talksage-notes/       # 模板化纪要生成（标准/谈判/评审/站会）
│   ├── talksage-server/      # headless 服务（axum API + WS + SPA 托管）
│   └── talksage-cli/         # launcher：web / serve / import / listen / doctor
├── web/
│   ├── src/                  # React SPA（转写分区、监听控制）
│   └── src-tauri/            # Tauri 适配器（command + 事件桥接）
├── scripts/                  # 模型下载、图标生成、一键测试
├── docs/                     # 架构 / PoC 报告 / 测试文档
└── models/                   # ASR/VAD 模型（gitignore，由脚本下载）
```

## 快速开始

### 环境

- Rust 1.85+（MSVC）、Node 18+、cmake（仅 sherpa-onnx 构建期，可选）
- 模型：`python scripts/download_models.py all`（经代理下载 streaming 模型 + silero VAD，约 310MB）

### 开发运行（Tauri GUI）

```bash
# 构建期依赖（sherpa-onnx 预编译静态库）
$env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\.tools\sherpa-onnx-archives"
cd web && npx tauri dev     # 窗口打开后点「开始监听」对着麦克风说话
```

### Headless 验证（无需 GUI）

```bash
cargo build -p talksage-cli
target\debug\talksage.exe doctor                        # 环境诊断
target\debug\talksage.exe listen --input mic            # 真实麦克风实时转写
target\debug\talksage.exe listen --input <中文16k.wav>  # 文件模拟（自动化验证）
target\debug\talksage.exe listen --input <中文16k.wav> --client <英文16k.wav>   # 双流
target\debug\talksage.exe listen --input <16k.wav> --save      # 落库
target\debug\talksage.exe import <16k.wav>              # 导入转写 → 新会话
```

### Headless 服务（多设备/浏览器）

```bash
# 先构建前端
cd web && npm run build && cd ..

# 启动服务（默认 127.0.0.1:8080；浏览器打开 http://127.0.0.1:8080）
target\debug\talksage.exe serve

# 局域网/手机访问（需 token 保护）
$env:TALKSAGE_SERVER_TOKEN = "mytoken"
target\debug\talksage.exe serve --host 0.0.0.0 --port 8080
# 浏览器访问 http://<本机IP>:8080/?token=mytoken
```

### 打包分发

```bash
cd web && npm run tauri build     # Windows → NSIS 安装包；macOS → dmg
```

## 测试

```bash
scripts\run_tests.ps1        # cargo test --workspace + vitest run（一键全量）
```

详见 [docs/testing.md](docs/testing.md)。

## 文档

- [架构设计](docs/architecture-v2.md)（双载体、延迟预算、模块划分）
- [ASR PoC 报告](docs/poc-asr-report.md)（延迟实测）
- [测试文档](docs/testing.md)
- [编译与打包指南](docs/BUILDING.md)（环境准备 / 依赖下载 / 编译 / 测试 / 打包 / 故障排除）
- [日志与调试指南](docs/LOGGING.md)（结构化日志位置 / 级别 / AI Agent 分析指引）

## 里程碑

- ✅ M0 骨架（workspace + launcher + Tauri 壳 + IPC hello-world）
- ✅ M1 实时转写闭环（采集 → VAD → 流式 ASR → 事件 → 前端）
- ✅ M1b 双流 + WASAPI 系统回环采集（Windows；macOS ScreenCaptureKit 待接入）→ 视频会议客户流可用
- ✅ M2 会议辅助核心（术语解释 / 简报检索 / 实时翻译插件 + 前端分区）
- ✅ M2 会话持久化（SQLite 落库 + 历史页搜索/详情）
- ✅ M3 纪要模板化（多模板 LLM 生成 + 保存会话）
- ✅ M3 导入转写（文件 → 离线 ASR → 新会话）
- ✅ M4 headless 服务（axum API + WS 事件 + SPA 托管，多设备浏览器访问）
- ⏳ 打包分发（NSIS/dmg，命令已就绪待 CI 构建）
- ⏳ 未来：macOS 回环（ScreenCaptureKit）、浏览器麦克风直传（WS 上行）

## 隐私

音频、转写、会话默认全部本地处理与存储；模型本地推理，无云端依赖。
