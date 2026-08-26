# whisper.cpp Vulkan GPU 后端：编译配置与问题记录

> 目标：在 Windows 上启用本地 GPU 语音识别（whisper.cpp Vulkan，AMD/Intel/NVIDIA 通吃），
> 与 Dictata 同款方案。本文记录代码实现、工具链安装、编译配置、踩坑与当前状态。

## 一、背景与目标

Windows 本地 ASR 此前仅 CPU / NVIDIA CUDA（sherpa-onnx），AMD/Intel 显卡无法加速、
大模型跑不动。参照 Dictata 的做法，接入 **whisper.cpp + Vulkan**：

- Vulkan 跨 AMD/Intel/NVIDIA（显卡驱动自带 loader，运行期无需额外 SDK）；
- 用 Whisper large-v3-turbo Q5_0（约 547 MiB，中文/中英混说鲁棒性好）；
- 与现有 sherpa-onnx 引擎共存：检测到 Vulkan GPU 时走 whisper.cpp，否则回退 CUDA/CPU/云端。

## 二、代码实现（已提交：`0379b03`）

| 文件 | 改动 |
|---|---|
| `crates/talksage-asr/src/gpu.rs` | `GpuBackend` 新增 `Vulkan` 变体；Windows detect 优先探测 Vulkan（`vulkan-1.dll`，受 `vulkan-gpu` feature 门控） |
| `crates/talksage-asr/src/metal.rs` | 泛化为 whisper.cpp GPU 适配器：macOS→Metal、Windows→Vulkan，日志按后端标注 |
| `crates/talksage-asr/src/routing.rs` | backend 支持 `vulkan`；auto 模式识别 Vulkan GPU |
| `crates/talksage-asr/src/lib.rs` | `create_engine_with_options` 在 Metal/Vulkan 时走 whisper.cpp 引擎；`vulkan-gpu` feature |
| `crates/talksage-asr/Cargo.toml` | 新增 `vulkan-gpu` feature → `whisper-rs/vulkan`；Windows x64 平台 optional whisper-rs 依赖 |
| `crates/talksage-pipeline/src/service.rs` | Metal/Vulkan 路由统一走 Whisper large-v3-turbo |
| `crates/talksage-pipeline/src/lib.rs` | 引擎池 provider 按平台（metal/vulkan）隔离 |
| `web/src-tauri/Cargo.toml` | Windows x64 启用 `vulkan-gpu` feature |
| `web/src/sections/SettingsSection.tsx` | ASR 本地推理后端加「Vulkan GPU（AMD/Intel/NVIDIA）」选项 |
| `docs/BUILDING.md`、`scripts/talksage.ps1` | 构建前置说明与 env 检测 |

## 三、工具链安装（已完成）

### 1. Vulkan SDK 1.4.357.0

- 下载：`https://sdk.lunarg.com/sdk/download/1.4.357.0/windows/vulkansdk-windows-X64-1.4.357.0.exe`
  （274 MB，SHA256 校验通过：`81F474711E9042F4CD22B31B2F7A8870DB2E428B21586FB43DD80150BE97310D`；
  URL 从 winget manifest `KhronosGroup.VulkanSDK` 获得）
- 静默安装：`install --accept-licenses --default-answer --confirm-command`（需 UAC 提权）
- 安装位置：`C:\VulkanSDK\1.4.357.0`，`vulkan-1.lib` 就位
- 环境变量 `VULKAN_SDK` 已自动写入系统级（`C:\VulkanSDK\1.4.357.0`）

### 2. LLVM 22.1.8（bindgen 需要 libclang）

- 下载：`https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.8/LLVM-22.1.8-win64.exe`
  （434 MB）
- 静默安装：`/S`（需 UAC 提权）
- 安装位置：`C:\Program Files\LLVM\bin\libclang.dll`
- 环境变量 `LIBCLANG_PATH` 需指向 `C:\Program Files\LLVM\bin`
  （用户级写入被沙箱拦截，当前会话手动设置可用；需持久化）

## 四、编译配置（关键）

whisper-rs-sys 会现场用 CMake 编译 whisper.cpp（GGML_VULKAN），需要以下环境：

```bat
@echo off
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set CARGO_HOME=D:\Work\aiproject\projects\talk-sage\.cargo-home
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
set VULKAN_SDK=C:\VulkanSDK\1.4.357.0
set HTTP_PROXY=
set HTTPS_PROXY=
set CARGO_TARGET_DIR=C:\wt
set SHERPA_ONNX_ARCHIVE_DIR=D:\Work\aiproject\projects\talk-sage\.tools\sherpa-onnx-archives
set CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded
set RUSTFLAGS=-C target-feature=+crt-static -C link-arg=/NODEFAULTLIB:msvcrt.lib -C link-arg=/NODEFAULTLIB:msvcp140.dll -C link-arg=/NODEFAULTLIB:msvcprt.lib -C link-arg=libcmt.lib -C link-arg=libcpmt.lib -C link-arg=libucrt.lib -C link-arg=libvcruntime.lib -C link-arg=legacy_stdio_definitions.lib
cd /d D:\Work\aiproject\projects\talk-sage
cargo build -p talksage-app
```

关键点：
1. **必须加载 VS 2022 开发环境**（`vcvars64.bat`）——否则 CMake 探测 C 编译器失败；
2. **必须用短 target 目录**（`CARGO_TARGET_DIR=C:\wt`）——whisper.cpp 的
   `vulkan-shaders-gen` 子项目路径超 Windows MAX_PATH（260 字符）会崩；
3. **`SHERPA_ONNX_ARCHIVE_DIR` 指向本地归档**——否则换 target 目录后 sherpa-onnx-sys
   会重新联网下载（直连 GitHub 超时）；
4. **`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`**——让 whisper.cpp 以静态 CRT（/MT）编译，
   与 sherpa-onnx 官方静态库一致（否则 MSVC 运行时双重链接 LNK2005）；
5. **`RUSTFLAGS` 强制静态 CRT**（`+crt-static` + 显式链接静态 CRT 库、禁用动态 CRT）。

## 五、编译踩坑记录

### 5.1 CMake 探测 C 编译器失败（已解决）
未加载 VS 环境时：`Check for working C compiler - broken`。
**解决**：`vcvars64.bat` 加载 MSVC 工具链。

### 5.2 vulkan-shaders-gen MAX_PATH 崩溃（已解决）
路径 `D:\Work\aiproject\projects\talk-sage\target\...` 过长，vulkan-shaders-gen 子项目
触发 Windows 260 字符限制。
**解决**：`CARGO_TARGET_DIR=C:\wt` 短路径（与 Dictata README 提示一致）。

### 5.3 sherpa-onnx 换目录后重新下载（已解决）
换 target 目录后，sherpa-onnx-sys 找不到归档去联网下载（GitHub 直连超时）。
**解决**：`SHERPA_ONNX_ARCHIVE_DIR` 指向 `.tools\sherpa-onnx-archives`（归档已就位）。

### 5.4 LNK1169 / LNK2005：MSVC 运行时双重链接（已解决）

**问题根源**：sherpa-onnx 官方预编译静态库以 `/MT`（静态 CRT）编译，
whisper.cpp 默认 `/MD`（动态 CRT），两者同时进入链接时 MSVC 运行时符号重定义。

**踩坑过程**：
- 第一次：`LNK2005` sherpa(/MT) vs whisper(/MD) → 运行时符号双重定义；
- 只加 `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded` 无效——cmake Rust crate 把 `-MD`
  直接写入 `CMAKE_C_FLAGS`，CMake policy CMP0091 默认 OLD 时 flags 比属性优先，
  `/MD` 仍胜出（CMakeCache.txt 里 `CMAKE_C_FLAGS_RELWITHDEBINFO` 含 `-MD` 即证）；
- Rust 侧加 `+crt-static` 后链接错误变为 `/NODEFAULTLIB` 相关冲突。

**最终解决方案**（`vendor/whisper-rs-sys/build.rs` 的 vulkan 分支，见 5.5）：
三层保险确保 whisper.cpp 编译为 /MT，配合 Rust 侧静态 CRT 统一运行时：

```
whisper.cpp  /MT ──┐
sherpa-onnx  /MT ──┼─→ 全静态 CRT，运行时无冲突
Rust         /MT  ─┘（RUSTFLAGS: +crt-static + /NODEFAULTLIB:msvcrt.lib 等）
```

### 5.5 whisper-rs-sys patch（已修复）
`vendor/whisper-rs-sys`（fork）+ 根 Cargo.toml `[patch.crates-io]`。

**根本原因**：cmake Rust crate 在生成 cmake 调用时会自动将 `-MD` 写入
`CMAKE_C_FLAGS` 和 `CMAKE_CXX_FLAGS`。CMake 缓存中虽有
`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`，但由于 CMake policy CMP0091 默认为 OLD
（即 `CMAKE_C_FLAGS_*` 中的显式 `/MD`/`/MT` 比该属性优先），`/MD` 最终胜出，
导致 patch 原先仅靠 `CMAKE_MSVC_RUNTIME_LIBRARY` 设置不够。

**修复方案（三层保险）**：
1. **CMake policy CMP0091=NEW**：`config.define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW")`
   ——让 CMake 把 `CMAKE_MSVC_RUNTIME_LIBRARY` 作为唯一 CRT 控制，不再把 `/MD` 写入
   `CMAKE_C_FLAGS_*` 初始值；
2. **显式设置静态 CRT**：`config.define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded")`；
3. **belt-and-suspenders**：`config.cflag("/MT")` + `config.cxxflag("/MT")`
   ——在 cmake crate 自动追加的 `-MD` 之后再追加 `/MT`（MSVC 以最后出现的 `/MD`|`/MT` 为准）。
4. **自动清除旧缓存**：build.rs 检测 CMakeCache.txt 含 `-MD` 时自动删除，
   无需手动 `cargo clean`，下次构建即触发正确的重新配置。

## 六、当前状态与下一步

**已完成**：
- ✅ Vulkan SDK 1.4.357.0 + LLVM 22.1.8 安装，SHA256 校验通过；
- ✅ `cargo check -p talksage-asr --features vulkan-gpu` 通过（whisper.cpp Vulkan 代码路径无编译错误）；
- ✅ `cargo check -p talksage-app`（含 vulkan feature）通过；
- ✅ 无 feature 的 fallback 路径全绿（asr 51 测试、pipeline 59 测试）；
- ✅ CRT 双链接问题根本原因查明并修复（`vendor/whisper-rs-sys/build.rs`，见 5.4、5.5）。

**待验证**（下次在完整构建环境下执行）：
- ⬜ 用四节中的 `.bat` 脚本执行 `cargo build -p talksage-app`，确认链接无 LNK 错误；
- ⬜ 构建成功后用 `dumpbin /directives` 验证 whisper.cpp 各 .lib 含 `DEFAULTLIB:LIBCMT`；
- ⬜ 验证 `GpuBackend::detect()` 在真实机器上返回 `Vulkan`（本机有 `vulkan-1.dll`）；
- ⬜ 用真实 Whisper large-v3-turbo Q5_0 模型跑一次转写，确认 GPU 推理正常。

**后续工作**：
1. 持久化 `LIBCLANG_PATH` 用户级环境变量（当前需每次手动设置；`setx LIBCLANG_PATH "C:\Program Files\LLVM\bin"`）；
2. 将构建批处理固化为 `scripts/build-vulkan.bat`（或集成进 `talksage.ps1` 的 build 子命令）；
3. 提交 `vendor/whisper-rs-sys` patch + 根 `Cargo.toml` patch 段（当前 `vendor/` 未跟踪）。
