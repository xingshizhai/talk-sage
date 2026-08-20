#!/usr/bin/env bash
# TalkSage v2 构建/运行工具（macOS / Linux）
#
# 用法:
#   ./scripts/talksage.sh env       # 环境检查
#   ./scripts/talksage.sh deps      # 下载依赖（模型 + sherpa 静态库 + 前端）
#   ./scripts/talksage.sh build     # 全量编译（debug）
#   ./scripts/talksage.sh dev       # Tauri 开发模式（热更新）
#   ./scripts/talksage.sh run       # 启动桌面应用（自动启动 Vite + Tauri）
#   ./scripts/talksage.sh doctor    # 运行环境/模型诊断
#   ./scripts/talksage.sh serve     # headless 服务
#   ./scripts/talksage.sh listen    # CLI 转写（麦克风）
#   ./scripts/talksage.sh test      # 全量测试
#   ./scripts/talksage.sh package   # 打包（dmg）
#   ./scripts/talksage.sh logs      # 查看日志
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 不覆盖用户的 CARGO_HOME：rustup 默认把工具链安装在 ~/.cargo，强制改到
# 项目目录会导致已安装的 cargo/rustc 消失。仅把依赖构建产物留在仓库内。
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export SHERPA_ONNX_ARCHIVE_DIR="$ROOT/.tools/sherpa-onnx-archives"
export TALKSAGE_DATA_DIR="$ROOT/.tools/data"
export TALKSAGE_MODELS_DIR="$ROOT/models"

# Homebrew keg-only rustup、Apple Silicon Homebrew 和 rustup 默认目录兼容。
for bin_dir in "$HOME/.cargo/bin" /opt/homebrew/opt/rustup/bin /usr/local/opt/rustup/bin /opt/homebrew/bin; do
    [ -d "$bin_dir" ] && PATH="$bin_dir:$PATH"
done
export PATH

CMD="${1:-help}"

env_check() {
    echo "=== 环境检查 ==="
    command -v rustc >/dev/null && echo "  [OK]   $(rustc --version)" || echo "  [MISS] rustc（macOS: brew install rustup && rustup default stable）"
    command -v cargo >/dev/null && echo "  [OK]   $(cargo --version)" || echo "  [MISS] cargo"
    command -v node >/dev/null && echo "  [OK]   node $(node --version)" || echo "  [MISS] node"
    command -v npm >/dev/null && echo "  [OK]   npm $(npm --version)" || echo "  [MISS] npm"
    xcode-select -p >/dev/null 2>&1 && echo "  [OK]   Xcode Command Line Tools" || echo "  [MISS] Xcode Command Line Tools（运行 xcode-select --install）"
    for m in \
        "models/sherpa-onnx-streaming-paraformer-zh" \
        "models/sherpa-onnx-streaming-zipformer-en-2023-06-26" \
        "models/silero-vad/silero_vad.onnx"; do
        [ -e "$ROOT/$m" ] && echo "  [OK]   $m" || echo "  [MISS] ${m}（运行 deps）"
    done
    ls "$SHERPA_ONNX_ARCHIVE_DIR"/sherpa-onnx-*.tar.bz2 >/dev/null 2>&1 \
        && echo "  [OK]   sherpa-onnx 静态库" \
        || echo "  [MISS] sherpa-onnx 静态库（运行 deps）"
    [ -d web/node_modules ] && echo "  [OK]   前端依赖" || echo "  [MISS] 前端依赖（运行 deps）"
}

deps() {
    echo "=== 下载依赖 ==="
    if ! ls "$SHERPA_ONNX_ARCHIVE_DIR"/sherpa-onnx-*.tar.bz2 >/dev/null 2>&1; then
        python3 scripts/download_sherpa.py --proxy "${https_proxy:-${HTTPS_PROXY:-}}"
    else
        echo "  sherpa 静态库已存在"
    fi
    python3 scripts/download_models.py all
    (cd web && npm install --ignore-scripts)
}

build() {
    echo "=== 全量编译 ==="
    # Tauri 的 build.rs 会在编译时嵌入 web/dist；必须先生成前端资源，
    # 否则直接运行 target/debug/talksage-app 只会显示空窗口。
    (cd web && npm run build)
    cargo build --workspace
}

release() {
    echo "=== Release 编译 ==="
    (cd web && npm run build)
    cargo build --workspace --release
}

require_file() {
    [ -e "$1" ] || { echo "缺少 $1；请先运行 ./scripts/talksage.sh build" >&2; exit 1; }
}

case "$CMD" in
    env)     env_check ;;
    deps)    deps ;;
    build)   build ;;
    release) release ;;
    dev)     (cd web && npm run tauri -- dev) ;;
    # cargo build 的 debug Tauri 二进制使用 tauri.conf.json 的 devUrl，不能脱离
    # Vite 单独启动；由 tauri dev 同时管理前端服务与原生进程。
    run)     (cd web && npm run tauri -- dev) ;;
    serve)   require_file "$CARGO_TARGET_DIR/debug/talksage"; "$CARGO_TARGET_DIR/debug/talksage" serve ;;
    listen)  require_file "$CARGO_TARGET_DIR/debug/talksage"; "$CARGO_TARGET_DIR/debug/talksage" listen --input mic ;;
    doctor)  require_file "$CARGO_TARGET_DIR/debug/talksage"; "$CARGO_TARGET_DIR/debug/talksage" doctor ;;
    test)
        # 固定测试日志级别，避免调用者的 RUST_LOG（例如 warn）使日志集成测试失真。
        RUST_LOG=debug TALKSAGE_LOG=debug cargo test --workspace
        (cd web && npm test)
        ;;
    package) (cd web && npm run tauri -- build) ;;
    logs)    ls -t "$TALKSAGE_DATA_DIR/logs"/talksage.log.* 2>/dev/null | head -1 | xargs -I{} sh -c 'echo "=== {} ==="; tail -50 "{}"' ;;
    *)       sed -n 's/^# //p' "$0" | head -20 ;;
esac
