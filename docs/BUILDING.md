# 拓思者（TalkSage）编译与打包指南

本指南说明如何从源码编译**拓思者**（产品中文名，英文 TalkSage，v2：Rust 全栈 + Tauri 2 + React）并打包成可安装的桌面应用（NSIS / MSI / dmg）。

**产品定位：** 个人 AI 会议助理——实时转写、说话人识别（先识别主人，再区分其他人）、术语解释、实时翻译、要点聚合、知识库简报、纪要生成，并内置录音与测试闭环。

**适用平台：** Windows 10/11（主推）、macOS 12+、Ubuntu 20.04+
**构建机器要求：** 内存 8GB+（模型推理与编译需要），磁盘 10GB+（Rust 依赖 + 模型 + 产物）

---

## 0. 快速上手（推荐：一键脚本）

仓库提供统一工具脚本，自动处理环境变量（CARGO_HOME / SHERPA_ONNX_ARCHIVE_DIR / TALKSAGE_DATA_DIR 项目内隔离）与路径解析：

```powershell
# Windows
.\scripts\talksage.ps1 env        # 环境检查（Rust/Node/模型/静态库）
.\scripts\talksage.ps1 deps       # 下载依赖（模型 + sherpa 静态库 + 前端）
.\scripts\talksage.ps1 build      # 全量编译（Rust + 前端）
.\scripts\talksage.ps1 dev        # Tauri 开发模式（热更新）
.\scripts\talksage.ps1 run        # 运行桌面 release 版
.\scripts\talksage.ps1 serve      # headless 服务（浏览器访问 127.0.0.1:8080）
.\scripts\talksage.ps1 listen     # CLI 实时转写（麦克风）
.\scripts\talksage.ps1 import -wav 录音.wav   # 导入转写入库
.\scripts\talksage.ps1 test       # 全量测试
.\scripts\talksage.ps1 package    # 打包（NSIS/MSI）
.\scripts\talksage.ps1 logs       # 查看最近日志
.\scripts\talksage.ps1 clean      # 清理构建产物

# macOS / Linux（首次运行）
chmod +x scripts/talksage.sh
./scripts/talksage.sh env       # 检查 Rust/Node/Xcode/模型
./scripts/talksage.sh deps      # 下载当前平台的 sherpa 库、模型和前端依赖
./scripts/talksage.sh build     # 编译 Rust workspace + React 前端
./scripts/talksage.sh doctor    # 验证运行目录和模型
./scripts/talksage.sh run       # 启动 Vite + macOS Tauri 应用（推荐本地运行方式）

# 开发热更新 / dmg 打包
./scripts/talksage.sh dev
./scripts/talksage.sh package
```

> 注意：不要直接运行 `target/debug/talksage-app`。普通 `cargo build` 的 Tauri
> debug 二进制会连接配置中的 `devUrl`，需要 Vite 开发服务器；直接执行只会得到
> 空窗口。使用 `talksage.sh run/dev`，或通过 `talksage.sh package` 生成可独立运行的应用。

> 脚本内部命令（`python`/`npm`/`npx`/`cargo`）需在 PATH 中；代理通过 `$env:https_proxy` / `$env:http_proxy` 设置后运行即可。
> 下面章节为手动步骤（脚本内部执行的等价操作），供定制/排障参考。

---

## 1. 前置环境

### 1.1 Rust 工具链（rustup + MSVC）

```powershell
# Windows：安装 rustup（默认 stable + MSVC toolchain）
# 若 rustup-init 未下载，可从 https://win.rustup.rs 获取
rustup-init.exe -y --default-toolchain stable --profile default

# 验证
rustc --version   # 1.85+ 均可
cargo --version
```

**Windows 必须安装 Visual Studio Build Tools（含 C++ 工作负载）**——Tauri/WebView2 与 sherpa-onnx 静态库链接需要 MSVC linker：
- 安装 [VS 2022 Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022)，勾选「使用 C++ 的桌面开发」工作负载
- 或已有 Visual Studio 2022 Community（含 C++ 工作负载）亦可

**macOS：**
```bash
xcode-select --install          # Command Line Tools（clang/linker）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

> **macOS 注意**
> - 首次运行点击「开始监听 / 录制我的声音」时，系统会弹出**麦克风权限**请求（已在 `web/src-tauri/Info.plist` 声明 `NSMicrophoneUsageDescription`）；拒绝后应用无法采集音频，需到 系统设置 → 隐私与安全性 → 麦克风 中开启。
> - **回环采集（客户语音）当前仅 Windows 支持**：macOS 上客户流自动降级为关闭（仅用户流单流运行），其余功能（转写/说话人/录音/回放/纪要）不受影响。

**Ubuntu：**
```bash
sudo apt install build-essential cmake libwebkit2gtk-4.1-dev \
  libappindicator3-dev librsvg2-dev patchelf libssl-dev portaudio19-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 1.2 Node.js

```powershell
# 需要 Node.js 18+（含 npm）
node --version
npm --version
```

### 1.3（Windows 可选）cmake

仅当不使用预编译 sherpa-onnx 库时需要；默认路径**不需要** cmake。

---

## 2. 网络与代理（重要）

本项目依赖下载 crates（crates.io）、模型（HuggingFace）、tauri 工具链（GitHub Releases）。

### 2.1 通用：设置代理（如有）

```powershell
$env:https_proxy = "http://127.0.0.1:10808"
$env:http_proxy  = "http://127.0.0.1:10808"
```

在此之后运行 `.\scripts\talksage.ps1 dev`（或已启动的桌面应用）时，**模型管理里的引擎下载**同样走这些代理（`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`）。Webhook 出站仍直连，不受代理影响。

### 2.2 本机 schannel TLS 故障时的 cargo 代理（仅 Windows 有此问题）

部分 Windows 机器 schannel 损坏（`SEC_E_NO_CREDENTIALS`），cargo/git/curl 全部 TLS 失败。此时使用仓库自带的**明文 HTTP 代理**：

1. 启动代理（端口 10810，后台保持运行）：
   ```powershell
   .\.venv\Scripts\python.exe scripts\cargo_http_proxy.py 10810
   # 或任意 python：python scripts\cargo_http_proxy.py 10810
   ```
2. 配置 cargo 使用本地镜像代理（**每个构建会话设置**）：
   ```powershell
   $env:CARGO_HOME = "$PWD\.cargo-home"      # 隔离的 cargo 缓存（可选）
   $env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\.tools\sherpa-onnx-archives"
   # .cargo-home/config.toml 内容（首次自动创建）：
   #   [source.crates-io]
   #   replace-with = "crates-io-http"
   #   [source.crates-io-http]
   #   registry = "sparse+http://127.0.0.1:10810/index/"
   #   [http]
   #   timeout = 600
   #   low-speed-limit = 1
   ```
   > 代理脚本会把 crates.io 请求映射到本地明文端口，再由代理进程经外层代理（10808）访问官方源。**代理进程需保持运行**。

> 若你的机器 TLS 正常，可跳过 2.2，直接使用官方 crates.io。

---

## 3. 获取源码与依赖

### 3.1 克隆仓库

```bash
git clone <repo-url> talk-sage
cd talk-sage
```

### 3.2 下载 sherpa-onnx 预编译静态库（构建期必需，约 120MB）

sherpa-onnx 的 Rust 绑定（`sherpa-onnx-sys`）在构建时从 GitHub Releases 自动下载 `sherpa-onnx-v1.13.5-win-x64-static-MT-Release-lib.tar.bz2`（Linux/macOS 同理）。**两种方式：**

**方式 A（推荐）：手动预置**（网络不稳时更可控）
```powershell
mkdir -Force .tools\sherpa-onnx-archives
# 用浏览器/下载工具下载到该目录：
#   https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.5/sherpa-onnx-v1.13.5-win-x64-static-MT-Release-lib.tar.bz2
$env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\.tools\sherpa-onnx-archives"
```

**方式 B：让构建自动下载**（需要能访问 GitHub Releases）

### 3.3 下载运行模型

推荐在应用左侧进入「模型管理」按需下载。应用会选择可写模型目录，执行磁盘空间预检、断点续传、完整性校验，并把下载进度与失败原因写入日志。目录解析和模型状态定义见 [模型管理架构](model-management.md)。

批量构建或离线准备也可以使用命令行：

```powershell
# 需要 Python 3（任意环境，脚本用标准库）
python scripts\download_models.py all
# 下载到 models/
#   sherpa-onnx-qwen3-asr-0.6b/             （CUDA/CPU 段级 ASR，约 878 MB）
#   whisper.cpp-large-v3-turbo-q5_0/       （Apple Silicon Metal ASR，约 547 MiB）
#   silero-vad/silero_vad.onnx              （VAD）
#   wespeaker/                               （实时声纹）
#   diarization/                             （会后分离）

# 旧基准模型不会被 all 下载；仅在维护历史测试时显式执行：
# python scripts\download_models.py legacy
```

> 声纹模型用于「多人讲话者区分」：模型存在时即可在线聚类为「讲话者/客户1/客户2/…」；设置页注册主人声音是可选增强，用于把匹配身份标记为「我」。模型未下载时保持原双流标签。
>
> 标点恢复模型由应用模型管理单独下载（约 294 MB）。主源为 sherpa-onnx 官方 GitHub Release，公共 Hugging Face 仓库只作为回退；不再使用会返回 401 的旧 `vocab500k` 地址。

### 3.4 前端依赖

```powershell
cd web
npm install --ignore-scripts    # 需 Node 18+；tauri CLI 二进制为平台包自动就位
cd ..
```

---

## 4. 编译（开发模式）

### 4.1 后端（Rust workspace）

```powershell
# 一次性环境变量（每个新终端都要设置）
$env:CARGO_HOME = "$PWD\.cargo-home"
$env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\.tools\sherpa-onnx-archives"

# 编译检查（快）
cargo check --workspace

# 完整编译（debug）
cargo build --workspace
# 产物：
#   target\debug\talksage.exe        CLI（launcher）
#   target\debug\talksage-app.exe    Tauri 桌面应用
```

### 4.2 前端（React SPA）

```powershell
cd web
npm run build        # tsc 类型检查 + vite 产物 → web/dist
```

### 4.3 运行

```powershell
# 桌面应用（开发热更新）
cd web
npx tauri dev

# 或 CLI 验证（无 GUI）
target\debug\talksage.exe doctor
target\debug\talksage.exe listen --input <16kHz中文.wav>
target\debug\talksage.exe serve          # headless 服务 → http://127.0.0.1:8080
```

---

## 5. 测试

```powershell
# 一键全量（Rust 单元 + 集成 + 前端 Vitest）
scripts\run_tests.ps1

# 或分开跑
$env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\.tools\sherpa-onnx-archives"
cargo test --workspace
cd web && npx vitest run
```

> 集成测试需要 `models/`（3.3）；模型缺失时自动跳过并打印提示，不失败。

### 5.2 日志调试

```powershell
# 结构化日志（JSON lines）写入 <数据目录>/logs/talksage.YYYY-MM-DD.log
# 级别：--verbose / --log-level / RUST_LOG / TALKSAGE_LOG
.\target\debug\talksage.exe --verbose listen --input a.wav
Get-Content "$env:TALKSAGE_DATA_DIR\logs\talksage.*.log" | Select-String '"level":"(ERROR|WARN)"'
```

详见 [docs/LOGGING.md](LOGGING.md)（日志位置、字段、AI Agent 分析指引）。

---

## 6. 打包（发布安装包）

### 6.1 Windows（NSIS + MSI）

```powershell
cd web
npx tauri build
```

- 自动执行 `npm run build`（前端）→ `cargo build --release`（约 8–10 分钟）→ 下载 WIX/NSIS 工具链并打包
- 产物（**workspace 根 `target/`**，非 web 下）：
  - `target/release/bundle/nsis/TalkSage_0.1.1_x64-setup.exe`（**推荐分发**，双击安装）
  - `target/release/bundle/msi/TalkSage_0.1.1_x64_en-US.msi`
  - `target/release/talksage-app.exe`（绿色版）

### 6.2 macOS（dmg）

```bash
./scripts/talksage.sh doctor
./scripts/talksage.sh build
./scripts/talksage.sh package
```

- 日常运行：`./scripts/talksage.sh run`
- 全量测试：`./scripts/talksage.sh test`
- Rust 构建产物统一位于 workspace 根 `target/`；不要为 `web/src-tauri` 另设 target 目录，否则会重复占用磁盘
- macOS 当前支持麦克风和文件输入；系统音频回环尚未实现，不会静默改用第二路麦克风

### 6.3 打包说明

- 打包需要能访问 GitHub Releases（tauri 自动下载 WIX/NSIS 工具链）
- 安装包**不包含模型**（约 310MB）；首次运行请将 `models/` 置于可执行文件同级或 `%APPDATA%/TalkSage`，或设置 `TALKSAGE_MODELS_DIR` 指向模型目录
- 安装包默认全功能：实时转写（需模型）、插件/纪要（需在设置页配置 LLM API Key）、headless 服务（`talksage serve`）

---

## 7. 环境变量参考

| 变量 | 用途 | 默认 |
|---|---|---|
| `CARGO_HOME` | cargo 缓存/配置目录（隔离构建用） | `~/.cargo` |
| `SHERPA_ONNX_ARCHIVE_DIR` | sherpa-onnx 预编译库目录 | 自动下载 |
| `TALKSAGE_MODELS_DIR` | ASR/VAD 模型根目录 | 相对可执行文件探测 |
| `TALKSAGE_DATA_DIR` | 数据目录（sessions.db 等），外部已设则优先 | 脚本运行：项目内 `config/`；直接运行程序：`~/.talksage` |
| `TALKSAGE_SERVER_TOKEN` | headless 服务鉴权 token | 空（不鉴权） |
| `TALKSAGE_SERVER_PORT` / `HOST` | 覆盖服务端口/绑定 | 8080 / 127.0.0.1 |
| `TALKSAGE_WEB_DIST` | SPA 静态目录（serve 用） | `web/dist` |
| `https_proxy` / `http_proxy` | 下载走代理 | — |

---

## 8. 故障排除

| 现象 | 处理 |
|---|---|
| cargo 下载失败 `schannel: SEC_E_NO_CREDENTIALS` | 见 §2.2（用 `scripts/cargo_http_proxy.py` + 本地代理） |
| `GetFrames ... 0 + 45 > 19`（sherpa-onnx） | 流式解码须 `while is_ready() { decode() }`；确认使用仓库 `talksage-asr` 封装（已处理） |
| 前端 `spawn EPERM`（esbuild） | 杀毒软件/受限环境拦截子进程；临时禁用或换环境 |
| `未找到 models/` | 运行 §3.3 下载模型，或设 `TALKSAGE_MODELS_DIR` |
| 打包时 WIX/NSIS 下载失败 | 检查代理；或 `npx tauri build --no-bundle`（仅产出 exe） |
| 中文语音识别质量差 | 确认使用 `paraformer-zh`（用户流）与 16kHz mono 输入 |
| `talksage serve` 打不开 | 先 `cd web && npm run build` 生成 `web/dist` |

---

## 9. 目录速览

```
talk-sage/
├── crates/                  # Rust workspace（core/config/asr/audio/pipeline/llm/
│                            #   knowledge/plugins/session/notes/server/cli）
├── web/                     # React SPA + src-tauri（Tauri 壳）
├── scripts/                 # 模型下载 / 图标生成 / cargo 代理 / 一键测试
├── models/                  # ASR/VAD 模型（运行时，gitignore）
├── docs/                    # 架构 / 测试 / PoC 报告
└── target/                  # Rust 构建产物（debug/release、bundle/）
```

详细架构见 `docs/architecture-v2.md`，测试分层、模型评估与发布门禁见 `docs/testing.md`。
