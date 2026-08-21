#!/usr/bin/env python3
"""TalkSage 可重复评估：语料准备、ASR 横评、真实音频设备冒烟与报告。"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import time
import wave

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = ROOT / "evaluation/evaluation.json"
SUMMARY_RE = re.compile(
    r"平均: RTF=(?P<rtf>[0-9.]+)(?:\s+CER/WER=(?P<error>[0-9.]+)%)?\s+首词延迟=(?P<latency>[0-9.]+)ms"
)
HYPOTHESIS_RE = re.compile(r"^  识别: (.*)$", re.MULTILINE)


def load_json(path: Path) -> dict:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def save_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


def cli_path() -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return ROOT / "target/debug" / f"talksage{suffix}"


def prepare_corpus(config: dict) -> list[str]:
    corpus = ROOT / config["corpus_dir"]
    manifest = load_json(corpus / "manifest.json")
    sources = {
        "sherpa-zh-mixed-001": ROOT / "models/sherpa-onnx-streaming-paraformer-zh/0.wav",
        "sherpa-en-reading-001": ROOT / "models/sherpa-onnx-streaming-zipformer-en-2023-06-26/0.wav",
    }
    messages = []
    for sample in manifest["samples"]:
        target = corpus / sample["audio"]
        source = sources.get(sample["id"])
        if target.is_file():
            messages.append(f"ready {sample['id']}: {target.relative_to(ROOT)}")
        elif source and source.is_file():
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            messages.append(f"copied {sample['id']}: {target.relative_to(ROOT)}")
        else:
            messages.append(f"missing {sample['id']}: run ./scripts/talksage.sh deps")
    return messages


def installed_engines(config: dict) -> tuple[list[str], list[str]]:
    model_dirs = {
        "paraformer-zh": "sherpa-onnx-streaming-paraformer-zh",
        "zipformer-en": "sherpa-onnx-streaming-zipformer-en-2023-06-26",
        "whisper-base": "sherpa-onnx-whisper-base",
        "whisper-small": "sherpa-onnx-whisper-small",
        "qwen3-asr": "sherpa-onnx-qwen3-asr-0.6b",
    }
    installed, skipped = [], []
    for engine in config["engines"]:
        if (ROOT / "models" / model_dirs[engine]).is_dir():
            installed.append(engine)
        else:
            skipped.append(engine)
    return installed, skipped


def materialize_flat_corpus(config: dict, temp: Path) -> int:
    corpus = ROOT / config["corpus_dir"]
    manifest = load_json(corpus / "manifest.json")
    count = 0
    temp.mkdir(parents=True, exist_ok=True)
    for sample in manifest["samples"]:
        audio = corpus / sample["audio"]
        reference = corpus / sample["reference"]
        if not audio.is_file() or not reference.is_file():
            continue
        shutil.copyfile(audio, temp / f"{sample['id']}.wav")
        shutil.copyfile(reference, temp / f"{sample['id']}.txt")
        count += 1
    return count


def run_asr(config: dict, requested: list[str] | None) -> dict:
    executable = cli_path()
    if not executable.is_file():
        raise RuntimeError("缺少 target/debug/talksage，请先运行 ./scripts/talksage.sh build")
    installed, skipped = installed_engines(config)
    engines = requested or installed
    unknown = sorted(set(engines) - set(config["engines"]))
    if unknown:
        raise RuntimeError(f"未知模型: {', '.join(unknown)}")
    work = ROOT / "target/evaluation-corpus"
    if work.exists():
        shutil.rmtree(work)
    if materialize_flat_corpus(config, work) == 0:
        raise RuntimeError("评估语料为空，请先运行 evaluate.py prepare")

    manifest = load_json(ROOT / config["corpus_dir"] / "manifest.json")
    samples = sorted(manifest["samples"], key=lambda sample: sample["id"])
    results = []
    env = os.environ.copy()
    env["TALKSAGE_MODELS_DIR"] = str(ROOT / "models")
    for engine in engines:
        if engine not in installed:
            results.append({"engine": engine, "status": "skipped", "reason": "model_not_installed"})
            continue
        started = time.perf_counter()
        proc = subprocess.run(
            [str(executable), "bench", "--dir", str(work), "--engine", engine],
            cwd=ROOT, env=env, text=True, capture_output=True,
        )
        output = proc.stdout + proc.stderr
        match = SUMMARY_RE.search(output)
        item = {
            "engine": engine,
            "status": "ok" if proc.returncode == 0 and match else "failed",
            "wall_ms": round((time.perf_counter() - started) * 1000, 1),
            "exit_code": proc.returncode,
            "output": output,
        }
        if match:
            item.update({
                "error_rate": None if match.group("error") is None else float(match.group("error")) / 100,
                "rtf": float(match.group("rtf")),
                "first_final_ms": float(match.group("latency")),
            })
            hypotheses = HYPOTHESIS_RE.findall(output)
            expected = recognized = predicted = correct_predictions = 0
            vocabulary = sorted({term for sample in samples for term in sample.get("terms", [])})
            for sample, hypothesis in zip(samples, hypotheses):
                reference = (ROOT / config["corpus_dir"] / sample["reference"]).read_text(encoding="utf-8")
                for term in sample.get("terms", []):
                    in_reference = term.casefold() in reference.casefold()
                    in_hypothesis = term.casefold() in hypothesis.casefold()
                    expected += int(in_reference)
                    recognized += int(in_reference and in_hypothesis)
                for term in vocabulary:
                    in_reference = term.casefold() in reference.casefold()
                    in_hypothesis = term.casefold() in hypothesis.casefold()
                    predicted += int(in_hypothesis)
                    correct_predictions += int(in_reference and in_hypothesis)
            item["term_recall"] = recognized / expected if expected else None
            item["term_precision"] = correct_predictions / predicted if predicted else None
            item["term_expected"] = expected
        results.append(item)
    return {"kind": "asr", "results": results, "not_installed": skipped}


def analyze_wav(path: Path, expected_seconds: float | None = None) -> dict:
    with wave.open(str(path), "rb") as wav:
        channels, rate, frames = wav.getnchannels(), wav.getframerate(), wav.getnframes()
        width = wav.getsampwidth()
        raw = wav.readframes(frames)
    if width != 2:
        raise RuntimeError(f"只支持 PCM16 诊断，实际 sample_width={width}")
    values = struct.unpack(f"<{len(raw) // 2}h", raw)
    mono = [sum(values[i:i + channels]) / channels / 32768.0 for i in range(0, len(values), channels)]
    duration = frames / rate if rate else 0.0
    rms = math.sqrt(sum(v * v for v in mono) / len(mono)) if mono else 0.0
    clipping = sum(abs(v) >= 0.99 for v in mono) / len(mono) if mono else 0.0
    block = max(1, rate // 50)
    quiet_blocks = sum(
        1 for i in range(0, len(mono), block)
        if max((abs(v) for v in mono[i:i + block]), default=0.0) < 0.001
    )
    blocks = math.ceil(len(mono) / block) if mono else 0
    return {
        "path": str(path), "sample_rate": rate, "channels": channels,
        "duration_seconds": round(duration, 3), "rms": round(rms, 6),
        "peak": round(max((abs(v) for v in mono), default=0.0), 6),
        "clipping_ratio": round(clipping, 6),
        "silence_ratio": round(quiet_blocks / blocks, 6) if blocks else 1.0,
        "capture_drift_ratio": None if not expected_seconds else round(abs(duration - expected_seconds) / expected_seconds, 6),
    }


def run_hardware(config: dict, seconds: int) -> dict:
    executable = cli_path()
    output_dir = ROOT / "target/evaluation-capture"
    output_dir.mkdir(parents=True, exist_ok=True)
    before = set(output_dir.glob("*.wav"))
    proc = subprocess.run(
        [str(executable), "record", "--seconds", str(seconds), "--dir", str(output_dir), "--input", "mic"],
        cwd=ROOT, text=True, capture_output=True,
    )
    created = sorted(set(output_dir.glob("*.wav")) - before, key=lambda p: p.stat().st_mtime)
    if proc.returncode != 0 or not created:
        return {"kind": "hardware", "status": "failed", "output": proc.stdout + proc.stderr}
    metrics = analyze_wav(created[-1], float(seconds))
    gates = config["quality_gates"]
    failures = []
    for metric, threshold in (
        ("capture_drift_ratio", gates["max_capture_drift_ratio"]),
        ("clipping_ratio", gates["max_clipping_ratio"]),
        ("silence_ratio", gates["max_silence_ratio"]),
    ):
        if metrics[metric] is not None and metrics[metric] > threshold:
            failures.append(f"{metric}={metrics[metric]} > {threshold}")
    return {"kind": "hardware", "status": "failed" if failures else "ok", "metrics": metrics, "failures": failures}


def score_and_gate(report: dict, config: dict) -> None:
    gates, weights = config["quality_gates"], config["score_weights"]
    for item in report.get("asr", {}).get("results", []):
        if item["status"] != "ok" or item.get("error_rate") is None:
            continue
        accuracy = max(0.0, 1.0 - item["error_rate"])
        realtime = max(0.0, 1.0 - min(item["rtf"] / gates["max_rtf"], 1.0))
        latency = max(0.0, 1.0 - min(item["first_final_ms"] / gates["max_first_final_ms"], 1.0))
        item["score"] = round(100 * (weights["accuracy"] * accuracy + weights["realtime"] * realtime + weights["latency"] * latency), 2)
        item["gate_failures"] = [
            name for name, failed in (
                ("error_rate", item["error_rate"] > gates["max_error_rate"]),
                ("rtf", item["rtf"] > gates["max_rtf"]),
                ("first_final_ms", item["first_final_ms"] > gates["max_first_final_ms"]),
            ) if failed
        ]
    candidates = [x for x in report.get("asr", {}).get("results", []) if "score" in x and not x["gate_failures"]]
    report["recommendation"] = max(candidates, key=lambda x: x["score"])["engine"] if candidates else None


def write_report(report: dict, config: dict) -> Path:
    score_and_gate(report, config)
    stamp = dt.datetime.now().astimezone().strftime("%Y%m%d-%H%M%S")
    path = ROOT / config["report_dir"] / f"evaluation-{stamp}.json"
    report["generated_at"] = dt.datetime.now().astimezone().isoformat()
    report["platform"] = sys.platform
    save_json(path, report)
    latest = path.parent / "latest.json"
    shutil.copyfile(path, latest)
    return path


def report_failed(report: dict, config: dict) -> bool:
    """候选模型失败不阻断 CI；只约束当前生产基线与显式硬件检查。"""
    baseline = config.get("baseline_engine")
    baseline_items = [
        item for item in report.get("asr", {}).get("results", [])
        if item.get("engine") == baseline
    ]
    asr_failed = bool(baseline_items) and (
        baseline_items[0].get("status") != "ok" or bool(baseline_items[0].get("gate_failures"))
    )
    no_viable_candidate = "asr" in report and report.get("recommendation") is None
    hardware_failed = report.get("hardware", {}).get("status") == "failed"
    return asr_failed or no_viable_candidate or hardware_failed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "asr", "hardware", "all"))
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--engines", help="逗号分隔；默认评估全部已安装模型")
    parser.add_argument("--seconds", type=int, default=5, help="真实麦克风采集时长")
    parser.add_argument("--hardware", action="store_true", help="all 时包含真实麦克风测试")
    args = parser.parse_args()
    config = load_json(args.config)
    if args.command == "prepare":
        print("\n".join(prepare_corpus(config)))
        return 0
    prepare_corpus(config)
    report = {}
    try:
        if args.command in ("asr", "all"):
            requested = args.engines.split(",") if args.engines else None
            report["asr"] = run_asr(config, requested)
        if args.command == "hardware" or (args.command == "all" and args.hardware):
            report["hardware"] = run_hardware(config, args.seconds)
    except RuntimeError as error:
        print(f"评估失败: {error}", file=sys.stderr)
        return 2
    path = write_report(report, config)
    print(json.dumps({"report": str(path), "recommendation": report.get("recommendation")}, ensure_ascii=False))
    return 1 if report_failed(report, config) else 0


if __name__ == "__main__":
    raise SystemExit(main())
