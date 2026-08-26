@echo off
REM TalkSage Windows Vulkan GPU 构建脚本
REM 用法: scripts\build-vulkan.bat [release]
REM 默认 debug 构建；传入 "release" 参数执行 release 构建。
REM
REM 前置条件:
REM   1. Vulkan SDK 1.4.357.0  (C:\VulkanSDK\1.4.357.0)
REM   2. LLVM 22.1.8           (C:\Program Files\LLVM\bin\libclang.dll)
REM   3. VS 2022 Community      (vcvars64.bat)

setlocal

REM ── 加载 MSVC 工具链 ─────────────────────────────────────────────────────────
set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%VCVARS%" (
    echo [错误] 未找到 VS 2022: %VCVARS%
    exit /b 1
)
call "%VCVARS%" >nul 2>&1

REM ── 检查前置工具 ──────────────────────────────────────────────────────────────
if not exist "C:\VulkanSDK\1.4.357.0\Lib\vulkan-1.lib" (
    echo [错误] 未找到 Vulkan SDK，请安装 VulkanSDK 1.4.357.0
    exit /b 1
)
if not exist "C:\Program Files\LLVM\bin\libclang.dll" (
    echo [错误] 未找到 LLVM libclang，请安装 LLVM 22.1.8
    exit /b 1
)

REM ── 环境变量 ──────────────────────────────────────────────────────────────────
set CARGO_HOME=%~dp0..\.cargo-home
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
set VULKAN_SDK=C:\VulkanSDK\1.4.357.0
set HTTP_PROXY=
set HTTPS_PROXY=
REM 短路径：避免 vulkan-shaders-gen 子项目触发 Windows MAX_PATH（260 字符）限制
set CARGO_TARGET_DIR=C:\wt
REM 本地 sherpa-onnx 归档，避免联网下载（GitHub 直连超时）
set SHERPA_ONNX_ARCHIVE_DIR=%~dp0..\.tools\sherpa-onnx-archives
REM 透传给 CMake，与 build.rs 的三层保险配合（重复无害）
set CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded
REM 全静态 CRT：+crt-static 让 Rust 侧也用 /MT，与 whisper.cpp 和 sherpa-onnx 一致
set RUSTFLAGS=-C target-feature=+crt-static -C link-arg=/NODEFAULTLIB:msvcrt.lib -C link-arg=/NODEFAULTLIB:msvcp140.dll -C link-arg=/NODEFAULTLIB:msvcprt.lib -C link-arg=libcmt.lib -C link-arg=libcpmt.lib -C link-arg=libucrt.lib -C link-arg=libvcruntime.lib -C link-arg=legacy_stdio_definitions.lib

REM ── 构建 ─────────────────────────────────────────────────────────────────────
cd /d "%~dp0.."

if /i "%1"=="release" (
    echo [构建] talksage-app ^(release^)...
    cargo build -p talksage-app --release
) else (
    echo [构建] talksage-app ^(debug^)...
    cargo build -p talksage-app
)

if %ERRORLEVEL% neq 0 (
    echo [失败] 构建报错，退出码 %ERRORLEVEL%
    exit /b %ERRORLEVEL%
)

echo [完成] 构建成功。产物: C:\wt\debug\talksage_app_lib.dll（或 release\）
endlocal
