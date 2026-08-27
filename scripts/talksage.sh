#!/usr/bin/env bash
# TalkSage v2 构建/运行工具（macOS / Linux）
#
# 用法:
#   ./scripts/talksage.sh bootstrap              # 一键：环境检查 + 下载依赖 + debug 编译
#   ./scripts/talksage.sh env                    # 环境检查（Rust/Node/Xcode/模型/静态库）
#   ./scripts/talksage.sh deps                   # 下载依赖（模型 + sherpa 静态库 + 前端）
#   ./scripts/talksage.sh build                  # 全量编译（Rust + 前端，debug）
#   ./scripts/talksage.sh build --release        # 全量编译 release（不打包 dmg）
#   ./scripts/talksage.sh release                # 同 build --release
#   ./scripts/talksage.sh dev                    # Tauri 开发模式（热更新，debug）
#   ./scripts/talksage.sh run                    # 运行桌面 debug 版
#   ./scripts/talksage.sh run --release          # 运行桌面 release 版
#   ./scripts/talksage.sh serve [-host H] [-port P]  # headless 服务
#   ./scripts/talksage.sh listen [-wav F] [-engine E] [-client C] [-save]
#   ./scripts/talksage.sh import -wav F [-engine E]  # 导入转写入库
#   ./scripts/talksage.sh trim -wav F [-out O] [-preset P]
#   ./scripts/talksage.sh record [-seconds N] [-dir D] [-input mic]
#   ./scripts/talksage.sh loop                   # 录音测试闭环（裁剪 + 回放）
#   ./scripts/talksage.sh doctor                 # 环境诊断
#   ./scripts/talksage.sh test                   # 全量测试
#   ./scripts/talksage.sh evaluate               # 固定语料横评已安装 ASR 模型
#   ./scripts/talksage.sh audio-test [秒]        # 真实麦克风采集质量测试
#   ./scripts/talksage.sh speaker-report         # 汇总声纹判定原因与相似度
#   ./scripts/talksage.sh package                # 打包（release + dmg + 升级签名）
#   ./scripts/talksage.sh logs                   # 查看最近日志
#   ./scripts/talksage.sh clean                  # 清理构建产物
#
# 构建模式:
#   debug   （build / run 默认; dev）: 编译快、无优化，适合开发调试；
#           build 同时产出 debug CLI（serve/listen 等）和可独立运行的 debug App（run 用）
#   release （build --release / package; run --release）: 全量优化；
#           必须显式加 --release 才构建/运行 release 版，不做自动降级
#
# 升级:
#   package 自动生成签名密钥并写入 tauri.conf.json 公钥；安装包带 .sig。
#   在线检查需配置真实更新源端点。macOS 离线升级使用 package 产出的 .dmg / .app。
#
# 环境变量（本脚本进程内）; 目录分离（v0.2+）:
#   TALKSAGE_CONFIG_DIR=$PWD/config   配置目录（只放 talksage.toml）
#   TALKSAGE_DATA_DIR=$PWD/data       数据目录（sessions.db / recordings / exports / …）
#   TALKSAGE_LOG_DIR=$PWD/logs        日志目录
#   TALKSAGE_MODELS_DIR=$PWD/models
#   外部已设的 TALKSAGE_* 一律沿用；未设才用项目内默认。
#   直接运行二进制（不经脚本）→ 程序默认 ~/.talksage。
#
# 代理: 设置 https_proxy / HTTPS_PROXY 后运行本脚本即可。
#
# macOS 麦克风: TCC 通常只给带 Info.plist 的 .app 弹授权框。
#   日常调试可用 dev（Tauri 开发包）；真采麦请 package 后运行 拓思者.app。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 不覆盖用户的 CARGO_HOME：rustup 默认把工具链装在 ~/.cargo。
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export SHERPA_ONNX_ARCHIVE_DIR="$ROOT/.tools/sherpa-onnx-archives"

DATA_DIR_EXTERNAL=0
if [ -n "${TALKSAGE_DATA_DIR:-}" ]; then
    DATA_DIR_EXTERNAL=1
    echo "提示: 沿用外部 TALKSAGE_DATA_DIR=$TALKSAGE_DATA_DIR（数据目录）"
else
    export TALKSAGE_DATA_DIR="$ROOT/data"
    echo "提示: 未检测到外部 TALKSAGE_DATA_DIR，脚本使用项目内数据目录 data/"
fi

if [ -z "${TALKSAGE_CONFIG_DIR:-}" ]; then
    if [ "$DATA_DIR_EXTERNAL" = 1 ]; then
        echo "提示: TALKSAGE_CONFIG_DIR 未设，配置文件位于 $TALKSAGE_DATA_DIR/talksage.toml"
    else
        export TALKSAGE_CONFIG_DIR="$ROOT/config"
        echo "提示: 配置文件目录 config/（talksage.toml 与数据分离）"
    fi
fi
if [ -z "${TALKSAGE_LOG_DIR:-}" ]; then
    export TALKSAGE_LOG_DIR="$ROOT/logs"
    echo "提示: 日志目录 logs/"
fi
export TALKSAGE_MODELS_DIR="$ROOT/models"

# Homebrew keg-only rustup、Apple Silicon Homebrew 和 rustup 默认目录。
for bin_dir in "$HOME/.cargo/bin" /opt/homebrew/opt/rustup/bin /usr/local/opt/rustup/bin /opt/homebrew/bin; do
    [ -d "$bin_dir" ] && PATH="$bin_dir:$PATH"
done
export PATH

CMD="${1:-help}"
if [ "$#" -gt 0 ]; then
    shift
fi
ARGS=("$@")

want_release() {
    local a
    for a in "${ARGS[@]+"${ARGS[@]}"}"; do
        case "$a" in --release|-release) return 0 ;; esac
    done
    return 1
}

has_switch() {
    local flag="$1" a
    for a in "${ARGS[@]+"${ARGS[@]}"}"; do
        [ "$a" = "$flag" ] && return 0
    done
    return 1
}

flag_value() {
    local flag="$1" i
    for ((i = 0; i < ${#ARGS[@]}; i++)); do
        if [ "${ARGS[$i]}" = "$flag" ] && [ $((i + 1)) -lt ${#ARGS[@]} ]; then
            printf '%s' "${ARGS[$((i + 1))]}"
            return 0
        fi
    done
    return 1
}

cli_debug() { printf '%s' "$CARGO_TARGET_DIR/debug/talksage"; }
cli_release() { printf '%s' "$CARGO_TARGET_DIR/release/talksage"; }
app_debug() { printf '%s' "$CARGO_TARGET_DIR/debug/talksage-app"; }
app_release() { printf '%s' "$CARGO_TARGET_DIR/release/talksage-app"; }

require_cli() {
    local exe
    exe="$(cli_debug)"
    if [ ! -e "$exe" ]; then
        echo "缺少 $exe；请先运行 ./scripts/talksage.sh build" >&2
        exit 1
    fi
}

# 旧版把 sessions.db / 录音 / 日志堆在配置目录；v0.2+ 迁到 data/ 与 logs/。
ensure_data_layout() {
    local data_dir="$TALKSAGE_DATA_DIR"
    local log_dir="$TALKSAGE_LOG_DIR"
    local cfg_dir="${TALKSAGE_CONFIG_DIR:-}"
    mkdir -p "$data_dir" "$log_dir"
    if [ -z "$cfg_dir" ] || [ "$cfg_dir" = "$data_dir" ]; then
        return 0
    fi
    local item src dst
    for item in sessions.db recordings exports voiceprints window.json tmp; do
        src="$cfg_dir/$item"
        [ -e "$src" ] || continue
        dst="$data_dir/$item"
        if [ -d "$src" ]; then
            if [ ! -e "$dst" ]; then
                mv "$src" "$dst"
                echo "  [migrate] $cfg_dir/$item → $data_dir/"
            else
                local name
                for src_child in "$src"/*; do
                    [ -e "$src_child" ] || continue
                    name="$(basename "$src_child")"
                    if [ ! -e "$dst/$name" ]; then
                        mv "$src_child" "$dst/"
                        echo "  [migrate] $item/$name → data/$item/"
                    else
                        echo "  [warn] $item/$name 与 data/$item/ 下同名文件并存（保留 data/ 下版本）"
                    fi
                done
            fi
        elif [ ! -e "$dst" ]; then
            mv "$src" "$dst"
            echo "  [migrate] $cfg_dir/$item → $data_dir/"
        else
            echo "  [warn] $cfg_dir/$item 与 $dst 同时存在（保留 data/ 下版本）"
        fi
    done
    local md
    for md in "$cfg_dir"/session-*.md; do
        [ -e "$md" ] || continue
        if [ ! -e "$data_dir/$(basename "$md")" ]; then
            mv "$md" "$data_dir/"
            echo "  [migrate] $(basename "$md") → data/"
        fi
    done
    local old_logs="$cfg_dir/logs"
    if [ -d "$old_logs" ]; then
        local moved=0 f t
        for f in "$old_logs"/*; do
            [ -f "$f" ] || continue
            t="$log_dir/$(basename "$f")"
            if [ ! -e "$t" ]; then
                mv "$f" "$t"
            else
                cat "$f" >> "$t"
                rm -f "$f"
            fi
            moved=1
        done
        if [ "$moved" = 1 ]; then
            echo "  [migrate] $cfg_dir/logs/* → $log_dir/（同名追加）"
        fi
        rmdir "$old_logs" 2>/dev/null || true
    fi
}
ensure_data_layout

ensure_dev_data() {
    local config_dir="${TALKSAGE_CONFIG_DIR:-$TALKSAGE_DATA_DIR}"
    local config="$config_dir/talksage.toml"
    local template="$ROOT/config/talksage.example.toml"
    mkdir -p "$config_dir"
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

env_check() {
    echo "=== 环境检查 ==="
    command -v rustc >/dev/null && echo "  [OK]   $(rustc --version)" || echo "  [MISS] rustc（macOS: brew install rustup && rustup default stable）"
    command -v cargo >/dev/null && echo "  [OK]   $(cargo --version)" || echo "  [MISS] cargo"
    command -v node >/dev/null && echo "  [OK]   node $(node --version)" || echo "  [MISS] node"
    command -v npm >/dev/null && echo "  [OK]   npm $(npm --version)" || echo "  [MISS] npm"
    if [ "$(uname -s)" = "Darwin" ]; then
        xcode-select -p >/dev/null 2>&1 && echo "  [OK]   Xcode Command Line Tools" || echo "  [MISS] Xcode Command Line Tools（运行 xcode-select --install）"
    fi
    local ok=1 m
    for m in \
        "models/silero-vad/silero_vad.onnx" \
        "models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"; do
        if [ -e "$ROOT/$m" ]; then
            echo "  [OK]   $m"
        else
            echo "  [MISS] ${m}（运行 deps）"
            ok=0
        fi
    done
    [ -e "$ROOT/models/sherpa-onnx-qwen3-asr-0.6b" ] \
        && echo "  [OK]   Qwen3-ASR 0.6B" \
        || echo "  [OPTIONAL] Qwen3-ASR 未安装（模型管理或 download_models.py qwen3-asr）"
    [ -e "$ROOT/models/whisper.cpp-large-v3-turbo-q5_0/ggml-large-v3-turbo-q5_0.bin" ] \
        && echo "  [OK]   Whisper large-v3-turbo" \
        || echo "  [OPTIONAL] Whisper Metal/Vulkan 模型未安装（download_models.py whisper-metal）"
    ls "$SHERPA_ONNX_ARCHIVE_DIR"/sherpa-onnx-*.tar.bz2 >/dev/null 2>&1 \
        && echo "  [OK]   sherpa-onnx 静态库" \
        || echo "  [MISS] sherpa-onnx 静态库（运行 deps）"
    [ -d web/node_modules ] && echo "  [OK]   前端依赖" || echo "  [MISS] 前端依赖（运行 deps）"
    if [ "$ok" != 1 ]; then
        echo
        echo "提示: 运行 ./scripts/talksage.sh deps 下载缺失依赖"
    fi
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
    local group
    for group in silero-vad wespeaker diarization; do
        python3 scripts/download_models.py "$group" --proxy "$proxy"
    done
    (cd web && npm install --ignore-scripts)
}

# 前端必须先编：tauri-build 在编译期嵌入 web/dist。
# App 单独开 custom-protocol，否则独立启动会去连 Vite（空白窗口）。
build() {
    local release=0
    if want_release; then
        release=1
        echo "=== 全量编译（release + 前端，不打包安装器）==="
    else
        echo "=== 全量编译（debug + 前端）==="
    fi
    echo
    echo "构建前端（web/dist）..."
    (cd web && npm run build)
    if [ "$release" = 1 ]; then
        cargo build --workspace --release --exclude talksage-app
        cargo build -p talksage-app --release --features "tauri/custom-protocol"
        echo
        echo "编译完成: $(app_release)（release App）+ $(cli_release)（release CLI）"
    else
        cargo build --workspace --exclude talksage-app
        cargo build -p talksage-app --features "tauri/custom-protocol"
        echo
        echo "编译完成: $(cli_debug)（debug CLI）+ $(app_debug)（debug App）"
    fi
    local cfg="${TALKSAGE_CONFIG_DIR:-$TALKSAGE_DATA_DIR}/talksage.toml"
    if [ ! -f "$cfg" ]; then
        echo "提示: 尚未初始化配置文件，运行 ./scripts/talksage.sh dev 会自动从模板创建:"
        echo "  $cfg"
    fi
}

cmd_dev() {
    ensure_dev_data
    echo "=== Tauri 开发模式 ==="
    (cd web && npx tauri dev)
}

cmd_run() {
    ensure_dev_data
    local exe
    if want_release; then
        exe="$(app_release)"
        if [ ! -e "$exe" ]; then
            echo "未找到 release 版（$exe），请先运行: ./scripts/talksage.sh build --release（或 package）" >&2
            exit 1
        fi
        echo "=== 运行桌面应用（release）==="
        exec "$exe"
    fi
    exe="$(app_debug)"
    if [ ! -e "$exe" ]; then
        echo "未找到 debug 版（$exe），请先运行: ./scripts/talksage.sh build" >&2
        exit 1
    fi
    echo "=== 运行桌面应用（debug）==="
    if [ "$(uname -s)" = "Darwin" ]; then
        echo "提示: 裸二进制通常没有 TCC 麦克风授权框；真采麦请 package 后打开 拓思者.app，或用 ./scripts/talksage.sh dev"
    fi
    exec "$exe"
}

cmd_serve() {
    ensure_dev_data
    require_cli
    local host="127.0.0.1" port="8080"
    host="$(flag_value -host || true)"
    port="$(flag_value -port || true)"
    [ -n "${host:-}" ] || host="127.0.0.1"
    [ -n "${port:-}" ] || port="8080"
    if [ ! -d "$ROOT/web/dist" ]; then
        echo "缺少 web/dist，先运行: ./scripts/talksage.sh build" >&2
        exit 1
    fi
    echo "=== headless 服务 http://${host}:${port} ==="
    exec "$(cli_debug)" serve --host "$host" --port "$port"
}

cmd_listen() {
    ensure_dev_data
    require_cli
    local wav engine client
    wav="$(flag_value -wav || true)"
    engine="$(flag_value -engine || true)"
    client="$(flag_value -client || true)"
    [ -n "${engine:-}" ] || engine="paraformer-zh"
    local args=(listen --input mic --engine "$engine")
    if [ -n "${wav:-}" ]; then
        args=(listen --input "$wav" --engine "$engine")
    fi
    if [ -n "${client:-}" ]; then
        args+=(--client "$client")
    fi
    if has_switch -save; then
        args+=(--save)
    fi
    echo "=== CLI 转写: ${args[*]} ==="
    exec "$(cli_debug)" "${args[@]}"
}

cmd_import() {
    ensure_dev_data
    require_cli
    local wav engine
    wav="$(flag_value -wav || true)"
    engine="$(flag_value -engine || true)"
    [ -n "${engine:-}" ] || engine="paraformer-zh"
    if [ -z "${wav:-}" ]; then
        echo "用法: ./scripts/talksage.sh import -wav <文件.wav> [-engine paraformer-zh|zipformer-en|qwen3-asr]" >&2
        exit 1
    fi
    echo "=== 导入转写: $wav ==="
    exec "$(cli_debug)" import "$wav" --engine "$engine"
}

cmd_trim() {
    require_cli
    local wav out preset
    wav="$(flag_value -wav || true)"
    out="$(flag_value -out || true)"
    preset="$(flag_value -preset || true)"
    [ -n "${preset:-}" ] || preset="standard"
    if [ -z "${wav:-}" ]; then
        echo "用法: ./scripts/talksage.sh trim -wav <录音.wav> [-out <输出.wav>] [-preset standard|sensitive|strict]" >&2
        exit 1
    fi
    echo "=== 静音裁剪: $wav ==="
    if [ -n "${out:-}" ]; then
        exec "$(cli_debug)" trim "$wav" -o "$out" --preset "$preset"
    fi
    exec "$(cli_debug)" trim "$wav" --preset "$preset"
}

cmd_record() {
    require_cli
    local seconds dir input
    seconds="$(flag_value -seconds || true)"
    dir="$(flag_value -dir || true)"
    input="$(flag_value -input || true)"
    [ -n "${seconds:-}" ] || seconds="30"
    [ -n "${input:-}" ] || input="mic"
    echo "=== 录制音频（不转写）: ${seconds}s input=$input ==="
    if [ -n "${dir:-}" ]; then
        exec "$(cli_debug)" record --seconds "$seconds" --dir "$dir" --input "$input"
    fi
    exec "$(cli_debug)" record --seconds "$seconds" --input "$input"
}

cmd_loop() {
    require_cli
    local rec_dir="$TALKSAGE_DATA_DIR/recordings"
    echo "=== 录音测试闭环（裁剪 + 回放验证）==="
    if [ ! -d "$rec_dir" ]; then
        echo "无录音目录: $rec_dir"
        exit 0
    fi
    local wav trimmed
    shopt -s nullglob
    for wav in "$rec_dir"/*.wav; do
        case "$wav" in *.trimmed.wav) continue ;; esac
        echo "--- $(basename "$wav") ---"
        "$(cli_debug)" trim "$wav" --preset standard || true
        trimmed="${wav%.wav}.trimmed.wav"
        if [ -f "$trimmed" ]; then
            "$(cli_debug)" listen --input "$trimmed" || true
        fi
    done
}

cmd_logs() {
    local log_dir="${TALKSAGE_LOG_DIR:-$TALKSAGE_DATA_DIR/logs}"
    if [ ! -d "$log_dir" ]; then
        echo "无日志目录: $log_dir"
        return 0
    fi
    local log
    log="$(ls -t "$log_dir"/talksage.*.log 2>/dev/null | head -1 || true)"
    if [ -z "$log" ]; then
        echo "无日志文件"
        return 0
    fi
    echo "=== $(basename "$log")（最近 50 行）==="
    tail -50 "$log"
}

cmd_clean() {
    echo "=== 清理构建产物 ==="
    local d
    for d in web/dist web/node_modules .cargo-home .tools; do
        if [ -e "$ROOT/$d" ]; then
            rm -rf "$ROOT/$d"
            echo "  已删 $d"
        fi
    done
    if [ -d "$CARGO_TARGET_DIR" ]; then
        if [ "$CARGO_TARGET_DIR" = "$ROOT/target" ]; then
            rm -rf "$CARGO_TARGET_DIR"
            echo "  已删 target"
        else
            rm -rf "$CARGO_TARGET_DIR/debug" "$CARGO_TARGET_DIR/release"
            echo "  已删 $CARGO_TARGET_DIR/{debug,release}"
        fi
    fi
    echo "清理完成（模型 models/ 保留）。"
}

write_updater_pubkey() {
    local pub_file="$1"
    command -v python3 >/dev/null || {
        echo "  [updater] 警告: 无 python3，请手动把公钥写入 web/src-tauri/tauri.conf.json plugins.updater.pubkey"
        return 0
    }
    python3 - "$pub_file" "$ROOT/web/src-tauri/tauri.conf.json" <<'PY'
import json, pathlib, re, sys
pub = pathlib.Path(sys.argv[1]).read_text().strip()
path = pathlib.Path(sys.argv[2])
text = path.read_text()
try:
    current = json.loads(text).get("plugins", {}).get("updater", {}).get("pubkey", "")
except json.JSONDecodeError as exc:
    print(f"  [updater] 警告: 自动写入公钥失败（{exc}），请手动把公钥填入 tauri.conf.json 的 plugins.updater.pubkey")
    raise SystemExit(0)
if current == pub:
    print("  [updater] 签名公钥已就绪")
    raise SystemExit(0)

def repl(match):
    return match.group(1) + pub + match.group(2)

new, n = re.subn(r'("pubkey"\s*:\s*")[^"]*(")', repl, text, count=1)
if n != 1:
    print("  [updater] 警告: 自动写入公钥失败，请手动把公钥填入 tauri.conf.json 的 plugins.updater.pubkey")
    raise SystemExit(0)
path.write_text(new)
print("  [updater] 已把签名公钥写入 web/src-tauri/tauri.conf.json")
PY
}

ensure_updater_keys() {
    local dir="$ROOT/.tools/updater"
    mkdir -p "$dir"
    local key_path="$dir/talksage-update.key"
    local pub_file="$key_path.pub"
    local pw_file="$dir/key.password"
    if [ ! -f "$key_path" ]; then
        local pw
        pw="$(python3 -c 'import secrets,string; a=string.ascii_letters+string.digits; print("".join(secrets.choice(a) for _ in range(24)))')"
        printf '%s' "$pw" > "$pw_file"
        echo "  [updater] 首次生成签名密钥: $key_path（公钥将写入 tauri.conf.json）"
        if ! (cd web && npx tauri signer generate -p "$pw" -w "$key_path" --ci); then
            echo "  [updater] 签名密钥生成失败（在线升级将不可用，可重跑 package 重试）"
            return 0
        fi
    fi
    export TAURI_SIGNING_PRIVATE_KEY_PATH="$key_path"
    if [ -f "$pw_file" ]; then
        export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(tr -d '\n' < "$pw_file")"
    fi
    if [ -f "$pub_file" ]; then
        write_updater_pubkey "$pub_file"
    fi
}

cmd_package() {
    ensure_updater_keys
    echo "=== 打包（tauri build：dmg/app + 升级签名）==="
    (cd web && npx tauri build)
    echo
    echo "产物:"
    local bundle="$CARGO_TARGET_DIR/release/bundle"
    if [ -d "$bundle" ]; then
        find "$bundle" -type f -print 2>/dev/null | while read -r f; do
            local mb
            mb="$(python3 -c "import os,sys; print(round(os.path.getsize(sys.argv[1])/1048576,1))" "$f")"
            echo "  ${f#"$ROOT/"}  (${mb} MB)"
        done
    fi
    local app="$bundle/macos/拓思者.app"
    if [ -d "$app" ]; then
        echo
        echo "macOS 应用: $app（麦克风授权在此包内才生效）"
        echo "运行: open \"$app\""
    fi
}

cmd_help() {
    # BSD sed（macOS /bin/bash 配套）不支持 \?；抽文件头注释到 set -euo 为止。
    sed -n '2,/^set -euo pipefail$/p' "$0" | sed '/^set -euo/d; s/^# //; s/^#//'
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
        echo "  ./scripts/talksage.sh doctor          # 运行期自检"
        echo "  ./scripts/talksage.sh run             # 桌面 debug 版（需先 build）"
        echo "  ./scripts/talksage.sh run --release   # 桌面 release 版"
        echo "  ./scripts/talksage.sh package         # 打 dmg / .app"
        echo "  ./scripts/talksage.sh serve           # 浏览器 http://127.0.0.1:8080"
        if [ "$(uname -s)" = "Darwin" ]; then
            echo
            echo "macOS 提示：麦克风是 TCC 保护资源，只有带 Info.plist 的 .app 才会弹授权框。"
            echo "            要真正采集麦克风，请 ./scripts/talksage.sh package 再 open 产出的 拓思者.app。"
        fi
        ;;
    env)     env_check ;;
    deps)    deps ;;
    build)   build ;;
    release) ARGS+=(--release); build ;;
    dev)     cmd_dev ;;
    run)     cmd_run ;;
    serve)   cmd_serve ;;
    listen)  cmd_listen ;;
    import)  cmd_import ;;
    trim)    cmd_trim ;;
    record)  cmd_record ;;
    loop)    cmd_loop ;;
    doctor)  require_cli; exec "$(cli_debug)" doctor ;;
    test)
        RUST_LOG=debug TALKSAGE_LOG=debug cargo test --workspace
        (cd web && npm test)
        python3 -m unittest discover -s scripts/tests -p 'test_*.py'
        ;;
    evaluate)
        require_cli
        python3 scripts/evaluate.py all
        ;;
    audio-test)
        require_cli
        python3 scripts/evaluate.py hardware --seconds "${1:-5}"
        ;;
    speaker-report)
        python3 scripts/speaker_report.py
        ;;
    package) cmd_package ;;
    logs)    cmd_logs ;;
    clean)   cmd_clean ;;
    *)       cmd_help ;;
esac
