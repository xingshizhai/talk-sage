#!/usr/bin/env bash
# TalkSage v2 构建/运行工具（macOS / Linux）
#
# 用法:
#   ./scripts/talksage.sh bootstrap # 一键：环境检查 + 下载依赖 + 编译（首次装机用这个）
#   ./scripts/talksage.sh env       # 环境检查
#   ./scripts/talksage.sh deps      # 下载依赖（模型 + sherpa 静态库 + 前端）
#   ./scripts/talksage.sh build     # 全量编译（debug）
#   ./scripts/talksage.sh dev       # Tauri 开发模式（热更新）
#   ./scripts/talksage.sh run       # 启动桌面应用（自动启动 Vite + Tauri）
#   ./scripts/talksage.sh doctor    # 运行环境/模型诊断
#   ./scripts/talksage.sh serve     # headless 服务
#   ./scripts/talksage.sh listen    # CLI 转写（麦克风）
#   ./scripts/talksage.sh test      # 全量测试
#   ./scripts/talksage.sh evaluate  # 准备固定语料并横评全部已安装 ASR 模型
#   ./scripts/talksage.sh audio-test [秒] # 真实麦克风采集质量与设备链路测试
#   ./scripts/talksage.sh speaker-report # 汇总声纹判定原因与相似度
#   ./scripts/talksage.sh package   # 打包（dmg）
#   ./scripts/talksage.sh logs      # 查看日志
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 不覆盖用户的 CARGO_HOME：rustup 默认把工具链安装在 ~/.cargo，强制改到
# 项目目录会导致已安装的 cargo/rustc 消失。仅把依赖构建产物留在仓库内。
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export SHERPA_ONNX_ARCHIVE_DIR="$ROOT/.tools/sherpa-onnx-archives"
# TALKSAGE_DATA_DIR: 外部已设则沿用（配置 + 数据目录）；未设时用程序默认
# 的 ~/.talksage。开发期如需项目内隔离，显式 export TALKSAGE_DATA_DIR 后再运行。
export TALKSAGE_DATA_DIR="${TALKSAGE_DATA_DIR:-$HOME/.talksage}"
DATA_DIR="$TALKSAGE_DATA_DIR"
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
        "models/silero-vad/silero_vad.onnx" \
        "models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"; do
        [ -e "$ROOT/$m" ] && echo "  [OK]   $m" || echo "  [MISS] ${m}（运行 deps）"
    done
    [ -e "$ROOT/models/sherpa-onnx-qwen3-asr-0.6b" ] \
        && echo "  [OK]   Qwen3-ASR 0.6B" \
        || echo "  [OPTIONAL] Qwen3-ASR 未安装（模型管理或 download_models.py qwen3-asr）"
    [ -e "$ROOT/models/whisper.cpp-large-v3-turbo-q5_0/ggml-large-v3-turbo-q5_0.bin" ] \
        && echo "  [OK]   Whisper large-v3-turbo Metal" \
        || echo "  [OPTIONAL] Apple Metal 模型未安装（模型管理或 download_models.py whisper-metal）"
    ls "$SHERPA_ONNX_ARCHIVE_DIR"/sherpa-onnx-*.tar.bz2 >/dev/null 2>&1 \
        && echo "  [OK]   sherpa-onnx 静态库" \
        || echo "  [MISS] sherpa-onnx 静态库（运行 deps）"
    [ -d web/node_modules ] && echo "  [OK]   前端依赖" || echo "  [MISS] 前端依赖（运行 deps）"
}

deps() {
    echo "=== 下载依赖 ==="
    local proxy="${https_proxy:-${HTTPS_PROXY:-}}"
    [ -n "$proxy" ] && echo "  使用代理: $proxy" || echo "  直连（如需代理: export https_proxy=http://127.0.0.1:10808）"
    if ! ls "$SHERPA_ONNX_ARCHIVE_DIR"/sherpa-onnx-*.tar.bz2 >/dev/null 2>&1; then
        python3 scripts/download_sherpa.py --proxy "$proxy"
    else
        echo "  sherpa 静态库已存在"
    fi
    # bootstrap 只下载音频链路公共模型。ASR 主模型体积较大，由应用模型管理页
    # 按平台下载；旧 Paraformer/Zipformer 仅用于诊断，不再重复安装。
    for group in silero-vad wespeaker diarization; do
        python3 scripts/download_models.py "$group" --proxy "$proxy"
    done
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

# 确保开发数据目录与配置文件存在（首次自动从模板初始化）
ensure_dev_data() {
    local config="$TALKSAGE_DATA_DIR/talksage.toml"
    local template="$ROOT/config/talksage.example.toml"
    if [ ! -d "$TALKSAGE_DATA_DIR" ]; then
        mkdir -p "$TALKSAGE_DATA_DIR"
        echo "已创建开发数据目录: $TALKSAGE_DATA_DIR"
    fi
    if [ ! -f "$config" ]; then
        if [ -f "$template" ]; then
            cp "$template" "$config"
            echo "已从模板初始化配置文件: $config"
            echo "提示: 编辑该文件填写 API Key 等配置（LLM 要点聚合 / 术语解释需要）"
        else
            echo "警告: 未找到配置模板 config/talksage.example.toml，将使用内置默认值运行"
        fi
    fi
}

case "$CMD" in
    bootstrap)
        env_check
        deps
        build
        echo
        echo "=== bootstrap 完成 ==="
        env_check
        echo
        echo "下一步："
        echo "  ./scripts/talksage.sh doctor   # 运行期自检（模型/配置/平台）"
        echo "  ./scripts/talksage.sh run      # 桌面应用（Vite + Tauri 开发模式）"
        echo "  ./scripts/talksage.sh serve    # 浏览器访问 http://127.0.0.1:8080"
        if [ "$(uname -s)" = "Darwin" ]; then
            echo
            echo "macOS 提示：麦克风是 TCC 保护资源，只有带 Info.plist 的 .app 包才会弹授权框。"
            echo "            要真正采集麦克风，请先 ./scripts/talksage.sh package 再运行产出的 .app。"
        fi
        ;;
    env)     env_check ;;
    deps)    deps ;;
    build)   build ;;
    release) release ;;
    dev)     ensure_dev_data; (cd web && npm run tauri -- dev) ;;
    # cargo build 的 debug Tauri 二进制使用 tauri.conf.json 的 devUrl，不能脱离
    # Vite 单独启动；由 tauri dev 同时管理前端服务与原生进程。
    run)     ensure_dev_data; (cd web && npm run tauri -- dev) ;;
    serve)   ensure_dev_data; require_file "$CARGO_TARGET_DIR/debug/talksage"; "$CARGO_TARGET_DIR/debug/talksage" serve ;;
    listen)  ensure_dev_data; require_file "$CARGO_TARGET_DIR/debug/talksage"; "$CARGO_TARGET_DIR/debug/talksage" listen --input mic ;;
    doctor)  require_file "$CARGO_TARGET_DIR/debug/talksage"; "$CARGO_TARGET_DIR/debug/talksage" doctor ;;
    test)
        # 固定测试日志级别，避免调用者的 RUST_LOG（例如 warn）使日志集成测试失真。
        RUST_LOG=debug TALKSAGE_LOG=debug cargo test --workspace
        (cd web && npm test)
        python3 -m unittest discover -s scripts/tests -p 'test_*.py'
        ;;
    evaluate)
        require_file "$CARGO_TARGET_DIR/debug/talksage"
        python3 scripts/evaluate.py all
        ;;
    audio-test)
        require_file "$CARGO_TARGET_DIR/debug/talksage"
        python3 scripts/evaluate.py hardware --seconds "${2:-5}"
        ;;
    speaker-report)
        python3 scripts/speaker_report.py
        ;;
    package)
        (cd web && npm run tauri -- build)
        # macOS 产物名取自 tauri.conf.json 的 productName
        bundle="$CARGO_TARGET_DIR/release/bundle/macos/拓思者.app"
        [ -d "$bundle" ] && echo "产物: $bundle（麦克风授权在此包内才生效）"
        ;;
    logs)    ls -t "$DATA_DIR/logs"/talksage.log.* 2>/dev/null | head -1 | xargs -I{} sh -c 'echo "=== {} ==="; tail -50 "{}"' ;;
    *)       sed -n 's/^# //p' "$0" | head -20 ;;
esac
