# scripts/talksage.local.ps1 — 本机环境覆盖（复制本文件并去掉 .example 后缀）
#
# 此文件会被 talksage.ps1 自动加载（dot-source），在 Ensure-VulkanEnv 之前执行。
# 已加入 .gitignore，修改不会入库——每台开发机各自维护自己的路径。
#
# 用途：Vulkan SDK / LLVM / Cargo 输出目录安装到非默认路径时，在此覆盖自动探测结果。
# 不需要修改的项保持注释即可。

# Vulkan SDK 安装目录（默认自动扫描 C:\VulkanSDK\*，取最新版）
# $env:VULKAN_SDK = "C:\VulkanSDK\1.4.357.0"

# LLVM/libclang 目录（bindgen 需要；默认探测 C:\Program Files\LLVM\bin）
# $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"

# Cargo 编译输出目录（默认 C:\wt，短路径避免 MAX_PATH 限制）
# C 盘空间不足时可改到其他短路径，例如 E:\wt
# $env:CARGO_TARGET_DIR = "C:\wt"

# 编译阶段是否保留代理（默认脚本会清除代理，让 Cargo 直连 rsproxy.cn 国内镜像）。
# 若你的网络访问 rsproxy.cn 必须走代理，取消注释并设为 "1"：
# $env:TALKSAGE_BUILD_KEEP_PROXY = "1"

# sherpa-onnx 本地归档目录（避免每次联网下载）
# $env:SHERPA_ONNX_ARCHIVE_DIR = "D:\Work\aiproject\projects\talk-sage\.tools\sherpa-onnx-archives"

# RUSTFLAGS：通常无需修改，Ensure-VulkanEnv 会自动设为全静态 CRT。
# 若需自定义（如关闭某个 /NODEFAULTLIB），在此覆盖：
# $env:RUSTFLAGS = "-C target-feature=+crt-static ..."
