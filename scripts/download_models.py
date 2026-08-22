"""下载 sherpa-onnx 模型（流式 ASR / 离线 ASR / VAD / 声纹）。

用法: python scripts/download_models.py [paraformer-zh | zipformer-en | whisper-base | whisper-small | qwen3-asr | silero-vad | wespeaker | diarization | all] [--proxy URL]

代理策略（默认直连）：
  1. --proxy 显式指定；2. 环境变量 https_proxy / HTTPS_PROXY；3. 都没有则直连。
下载到 models/<repo>/
"""
import os
import struct
import sys
import tarfile
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
    # 会后离线说话人分离：pyannote segmentation + 上面的 WeSpeaker embedding。
    # 官方归档约 7MB；只提取 int8 模型，避免保存重复的 fp32 模型和示例脚本。
    "diarization": [],
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
    # Qwen3-ASR 0.6B（离线，段级；中英等多语言）。官方 HF 仓库为 gated（401），
    # sherpa-onnx 官方发布 int8 版本到 GitHub release（含 conv_frontend / encoder.int8 /
    # decoder.int8 / tokenizer/ 目录，878MB）。这里下载 tar.bz2 解压到
    # models/sherpa-onnx-qwen3-asr-0.6b/（目录名与代码 EngineKind::model_dir_name 对齐）。
    "qwen3-asr": [],
}

DIARIZATION_ARCHIVE_URL = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2"
DIARIZATION_DIR = "sherpa-onnx-pyannote-segmentation-3-0"

# Qwen3-ASR 0.6B int8（sherpa-onnx 官方 GitHub release；HF 仓库 gated）。
# 解压后顶层目录 sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/，含
# conv_frontend.onnx / encoder.int8.onnx / decoder.int8.onnx / tokenizer/（vocab.json 等）。
QWEN3_ASR_ARCHIVE_URL = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2"
QWEN3_ASR_DIR = "sherpa-onnx-qwen3-asr-0.6b"


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


def download_diarization_model() -> None:
    out_dir = OUT_ROOT / DIARIZATION_DIR
    model = out_dir / "model.int8.onnx"
    if model.is_file() and model.stat().st_size > 0:
        print("  skip (exists) model.int8.onnx")
        return
    archive = OUT_ROOT / f"{DIARIZATION_DIR}.tar.bz2"
    download(None, archive.name, DIARIZATION_ARCHIVE_URL, ".")
    out_dir.mkdir(parents=True, exist_ok=True)
    member_name = f"{DIARIZATION_DIR}/model.int8.onnx"
    with tarfile.open(archive, "r:bz2") as bundle:
        member = bundle.getmember(member_name)
        source = bundle.extractfile(member)
        if source is None:
            raise RuntimeError(f"归档缺少 {member_name}")
        with open(model, "wb") as target:
            while chunk := source.read(262144):
                target.write(chunk)
    archive.unlink(missing_ok=True)
    print(f"  extracted model.int8.onnx ({model.stat().st_size / 1e6:.1f} MB)")


def download_qwen3_asr_model() -> None:
    """下载并解压 Qwen3-ASR 0.6B int8 到 models/sherpa-onnx-qwen3-asr-0.6b/。

    官方包顶层目录名带版本（sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25），
    这里统一解压到与代码约定一致的目录名（EngineKind::model_dir_name）。
    """
    out_dir = OUT_ROOT / QWEN3_ASR_DIR
    required = ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"]
    if all((out_dir / name).is_file() for name in required) and (out_dir / "tokenizer").is_dir():
        print("  skip (exists) qwen3-asr 0.6B int8")
        return
    archive = OUT_ROOT / "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2"
    download(None, archive.name, QWEN3_ASR_ARCHIVE_URL, ".")
    staging = OUT_ROOT / "sherpa-onnx-qwen3-asr-0.6b.staging"
    if staging.exists():
        import shutil
        shutil.rmtree(staging)
    staging.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:bz2") as bundle:
        # 顶层目录 strip：sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/* → staging/*
        prefix = None
        for member in bundle.getmembers():
            top = member.name.split("/", 1)[0]
            prefix = prefix or top
            new_name = member.name[len(prefix):].lstrip("/")
            if not new_name:
                continue
            if member.isdir():
                (staging / new_name).mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                source = bundle.extractfile(member)
                if source is None:
                    raise RuntimeError(f"归档缺少 {member.name}")
                target = staging / new_name
                target.parent.mkdir(parents=True, exist_ok=True)
                with open(target, "wb") as f:
                    while chunk := source.read(262144):
                        f.write(chunk)
            else:
                print(f"  warn: 跳过特殊条目 {member.name}")
    archive.unlink(missing_ok=True)
    # staging → 正式目录（若已存在旧版，先移除）
    if out_dir.exists():
        import shutil
        shutil.rmtree(out_dir)
    staging.rename(out_dir)
    total = sum(p.stat().st_size for p in out_dir.rglob("*") if p.is_file())
    print(f"  extracted qwen3-asr 0.6B int8 -> {out_dir} ({total / 1e6:.1f} MB)")


def _read_varint(data: bytes, pos: int) -> tuple[int, int]:
    value = 0
    shift = 0
    while pos < len(data):
        byte = data[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        if byte < 0x80:
            return value, pos
        shift += 7
        if shift > 63:
            break
    raise ValueError("无效 protobuf varint")


def _skip_field(data: bytes, pos: int, wire: int) -> int:
    if wire == 0:
        return _read_varint(data, pos)[1]
    if wire == 1:
        return pos + 8
    if wire == 2:
        size, pos = _read_varint(data, pos)
        return pos + size
    if wire == 5:
        return pos + 4
    raise ValueError(f"不支持的 protobuf wire type: {wire}")


def export_sentencepiece_vocab(model: Path, output: Path) -> None:
    """从 SentencePiece ModelProto 导出 sherpa 热词需要的 token/score 文本词表。"""
    data = model.read_bytes()
    pieces: list[tuple[str, float]] = []
    pos = 0
    while pos < len(data):
        tag, pos = _read_varint(data, pos)
        field, wire = tag >> 3, tag & 7
        if field != 1 or wire != 2:  # ModelProto.pieces
            pos = _skip_field(data, pos, wire)
            continue
        size, pos = _read_varint(data, pos)
        message, pos = data[pos : pos + size], pos + size
        piece = None
        score = 0.0
        inner = 0
        while inner < len(message):
            inner_tag, inner = _read_varint(message, inner)
            inner_field, inner_wire = inner_tag >> 3, inner_tag & 7
            if inner_field == 1 and inner_wire == 2:
                length, inner = _read_varint(message, inner)
                piece = message[inner : inner + length].decode("utf-8")
                inner += length
            elif inner_field == 2 and inner_wire == 5:
                score = struct.unpack_from("<f", message, inner)[0]
                inner += 4
            else:
                inner = _skip_field(message, inner, inner_wire)
        if piece is not None:
            pieces.append((piece, score))
    if not pieces:
        raise ValueError(f"SentencePiece 模型无词条: {model}")
    output.write_text("".join(f"{piece}\t{score:.9g}\n" for piece, score in pieces), encoding="utf-8")
    print(f"  generated {output.name} ({len(pieces)} pieces)")


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
            if name == "diarization":
                download_diarization_model()
            if name == "qwen3-asr":
                download_qwen3_asr_model()
            if name == "zipformer-en":
                model_dir = OUT_ROOT / "sherpa-onnx-streaming-zipformer-en-2023-06-26"
                bpe_model = model_dir / "bpe.model"
                bpe_vocab = model_dir / "bpe.vocab"
                if bpe_model.is_file() and not bpe_vocab.is_file():
                    export_sentencepiece_vocab(bpe_model, bpe_vocab)


if __name__ == "__main__":
    main()
