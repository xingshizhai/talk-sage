#!/usr/bin/env bash
# TalkSage v2 构建/运行工具（macOS / Linux）
#
# 用法:
#   ./scripts/talksage.sh env       # 环境检查
#   ./scripts/talksage.sh deps      # 下载依赖（模型 + sherpa 静态库 + 前端）
#   ./scripts/talksage.sh build     # 全量编译
#   ./scripts/talksage.sh dev       # Tauri 开发模式
#   ./scripts/talksage.sh serve     # headless 服务
#   ./scripts/talksage.sh listen    # CLI 转写（麦克风）
#   ./scripts/talksage.sh test      # 全量测试
#   ./scripts/talksage.sh package   # 打包（dmg）
#   ./scripts/talksage.sh logs      # 查看日志
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_HOME="$ROOT/.cargo-home"
export SHERPA_ONNX_ARCHIVE_DIR="$ROOT/.tools/sherpa-onnx-archives"
export TALKSAGE_DATA_DIR="$ROOT/.tools/data"
export TALKSAGE_MODELS_DIR="$ROOT/models"

CMD="${1:-help}"

env_check() {
    echo "=== 环境检查 ==="
    command -v rustc && rustc --version || echo "  [MISS] rustc"
    command -v node && node --version || echo "  [MISS] node"
    for m in \
        "models/sherpa-onnx-streaming-paraformer-zh" \
        "models/sherpa-onnx-streaming-zipformer-en-2023-06-26" \
        "models/silero-vad/silero_vad.onnx"; do
        [ -e "$ROOT/$m" ] && echo "  [OK]   $m" || echo "  [MISS] $m（运行 deps）"
    done
}

deps() {
    echo "=== 下载依赖 ==="
    if ! ls "$SHERPA_ONNX_ARCHIVE_DIR"/sherpa-onnx-*.tar.bz2 >/dev/null 2>&1; then
        python3 scripts/download_sherpa.py
    else
        echo "  sherpa 静态库已存在"
    fi
    python3 scripts/download_models.py all
    (cd web && npm install --ignore-scripts)
}

build() {
    echo "=== 全量编译 ==="
    cargo build --workspace
    (cd web && npm run build)
}

case "$CMD" in
    env)     env_check ;;
    deps)    deps ;;
    build)   build ;;
    dev)     (cd web && npx tauri dev) ;;
    serve)   cargo run -p talksage-cli -- serve ;;
    listen)  cargo run -p talksage-cli -- listen --input mic ;;
    test)    bash scripts/run_tests.sh 2>/dev/null || { cargo test --workspace; (cd web && npx vitest run); } ;;
    package) (cd web && npm run tauri build) ;;
    logs)    ls -t "$TALKSAGE_DATA_DIR/logs"/talksage.log.* 2>/dev/null | head -1 | xargs -I{} sh -c 'echo "=== {} ==="; tail -50 "{}"' ;;
    *)       sed -n 's/^# //p' "$0" | head -20 ;;
esac
