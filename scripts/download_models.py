"""下载 sherpa-onnx streaming 模型。

用法: python scripts/download_models.py [paraformer-zh | zipformer-en | wespeaker | all] [--proxy URL]

代理策略（默认直连）：
  1. --proxy 显式指定；2. 环境变量 https_proxy / HTTPS_PROXY；3. 都没有则直连。
下载到 models/<repo>/
"""
import os
import sys
import urllib.request
from pathlib import Path

BASE = "https://huggingface.co"
# 镜像：HF_ENDPOINT=https://hf-mirror.com 可切换到国内镜像
BASE = os.environ.get("HF_ENDPOINT", BASE).rstrip("/")
OUT_ROOT = Path(os.environ.get("TALKSAGE_MODELS_DIR") or Path(__file__).resolve().parent.parent / "models")


def build_opener(proxy: str) -> urllib.request.OpenerDirector:
    """proxy 为空 = 直连（ProxyHandler({}) 显式禁用系统代理探测）。"""
    return urllib.request.build_opener(
        urllib.request.ProxyHandler({"http": proxy, "https": proxy} if proxy else {})
    )


opener = build_opener("")

TARGETS: dict[str, list[tuple[str | None, str, str]]] = {
    "paraformer-zh": [
        # fp32（更准，约 300MB；引擎存在时优先使用）
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "encoder.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "decoder.onnx", ""),
        # int8（更小更快，作为后备）
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "encoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "decoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "tokens.txt", ""),
        # 集成测试音频：tests/pipeline_live.rs 期望 <model_dir>/0.wav，
        # 上游放在 test_wavs/ 下，这里用显式 URL 落到模型目录根。缺失时集成测试会静默跳过。
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "0.wav",
         "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-paraformer-zh/resolve/main/test_wavs/0.wav"),
    ],
    "zipformer-en": [
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "encoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "decoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "joiner-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "bpe.model", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "tokens.txt", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "0.wav",
         "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26/resolve/main/test_wavs/0.wav"),
    ],
    # VAD 模型：整条流水线的前置依赖（分段、静音裁剪、录音都要用），沿用 sherpa-onnx 官方分发版本。
    "silero-vad": [
        (None, "silero_vad.onnx", "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"),
    ],
    # 声纹模型（说话人识别）：GitHub release（sherpa-onnx speaker-recongition-models，官方 tag 拼写如此）
    "wespeaker": [
        (None, "wespeaker_zh_cnceleb_resnet34.onnx", "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_zh_cnceleb_resnet34.onnx"),
    ],
    # OpenAI Whisper（离线，段级识别；比流式更准，多语言）。fp32 + int8 都下载，引擎优先 fp32。
    "whisper-base": [
        ("csukuangfj/sherpa-onnx-whisper-base", "base-encoder.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-base", "base-decoder.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-base", "base-encoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-base", "base-decoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-base", "base-tokens.txt", ""),
    ],
    "whisper-small": [
        ("csukuangfj/sherpa-onnx-whisper-small", "small-encoder.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-small", "small-decoder.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-small", "small-encoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-small", "small-decoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-whisper-small", "small-tokens.txt", ""),
    ],
    # Qwen3-ASR 0.6B（离线，段级；中英等多语言）。官方仓库暂未公开，模型可用后放到
    # models/sherpa-onnx-qwen3-asr-0.6b/（conv_frontend.onnx / encoder.onnx / decoder.onnx / tokenizer.json）。
    "qwen3-asr": [],
}


def download(repo: str | None, filename: str, url: str, group: str) -> Path:
    """repo 为 None 时（直链下载）按目标组名建目录，与代码里的模型路径约定一致。"""
    out_dir = OUT_ROOT / (repo.split("/")[-1] if repo else group)
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / filename
    if out.exists() and out.stat().st_size > 0:
        print(f"  skip (exists) {filename}")
        return out
    if not url:
        url = f"{BASE}/{repo}/resolve/main/{filename}"
    print(f"  downloading {filename} ...", flush=True)
    with opener.open(url, timeout=900) as r, open(out, "wb") as f:
        while True:
            chunk = r.read(262144)
            if not chunk:
                break
            f.write(chunk)
    print(f"    -> {out} ({out.stat().st_size / 1e6:.1f} MB)")
    return out


def main() -> None:
    global opener
    args = sys.argv[1:]
    proxy = os.environ.get("https_proxy") or os.environ.get("HTTPS_PROXY") or ""
    if "--proxy" in args:
        i = args.index("--proxy")
        proxy = args[i + 1] if i + 1 < len(args) else ""
        del args[i : i + 2]
    opener = build_opener(proxy)

    want = args[0] if args else "all"
    if want not in TARGETS and want != "all":
        print(f"未知目标: {want}（可选: all, {', '.join(TARGETS)}）")
        raise SystemExit(2)
    print(f"源: {BASE}  代理: {proxy or '直连'}  输出: {OUT_ROOT}")
    for name, files in TARGETS.items():
        if want in ("all", name):
            print(f"== {name} ==")
            for repo, filename, url in files:
                download(repo, filename, url, name)


if __name__ == "__main__":
    main()
