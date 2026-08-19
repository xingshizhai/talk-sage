"""下载 sherpa-onnx streaming 模型（经代理 127.0.0.1:10808）。

用法: python scripts/download_models.py [paraformer-zh | zipformer-en | all]
下载到 models/<repo>/
"""
import os
import sys
import urllib.request
from pathlib import Path

PROXY = "http://127.0.0.1:10808"
BASE = "https://huggingface.co"
OUT_ROOT = Path(__file__).resolve().parent.parent / "models"

opener = urllib.request.build_opener(
    urllib.request.ProxyHandler({"http": PROXY, "https": PROXY})
)

TARGETS: dict[str, list[tuple[str | None, str, str]]] = {
    "paraformer-zh": [
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "encoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "decoder.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-paraformer-zh", "tokens.txt", ""),
    ],
    "zipformer-en": [
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "encoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "decoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "joiner-epoch-99-avg-1-chunk-16-left-64.int8.onnx", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "bpe.model", ""),
        ("csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26", "tokens.txt", ""),
    ],
    # 声纹模型（说话人识别）：GitHub release（sherpa-onnx speaker-recongition-models，官方 tag 拼写如此）
    "wespeaker": [
        (None, "wespeaker_zh_cnceleb_resnet34.onnx", "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_zh_cnceleb_resnet34.onnx"),
    ],
}


def download(repo: str | None, filename: str, url: str) -> Path:
    out_dir = OUT_ROOT / (repo.split("/")[-1] if repo else "wespeaker")
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
    want = sys.argv[1] if len(sys.argv) > 1 else "all"
    for name, files in TARGETS.items():
        if want in ("all", name):
            print(f"== {name} ==")
            for repo, filename, url in files:
                download(repo, filename, url)


if __name__ == "__main__":
    main()
