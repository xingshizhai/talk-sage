#!/usr/bin/env python3
"""诊断一次历史会话的采集电平和转写质量。

读取数据目录里的 sessions.db 与分轨 WAV，对照产品里的增益限幅、电平表、
质量阈值和场景强制切段，说明「电平很高但识别差」是削波、底噪还是切段造成的。

用法:
  python tools/session_diagnose.py --data-dir data --title JMH-1015
  python tools/session_diagnose.py --data-dir data --id 69
  python tools/session_diagnose.py --data-dir data --from-end 2
  python tools/session_diagnose.py --data-dir data --json
"""

from __future__ import annotations

import argparse
import json
import math
import os
from collections import Counter
from dataclasses import asdict, dataclass
from pathlib import Path
import re
import sqlite3
import struct
import sys
import wave


# 与产品实现对齐。改采集或电平表时，这里的门槛要一起改。
# 限幅 ±0.98：crates/talksage-audio/src/lib.rs collect_and_send
# 贴限幅计数用 |i16|>=32000（约 0.9766），钳位后的峰值是 0.98 附近而不是恰好 1.0。
# 电平 100% = RMS>=0.1，过载 = RMS>=0.2：web/src/components/SideNav.tsx
# 语音占比 >0.85 标噪音：QualityParams::default max_speech_ratio
# 非流式引擎按场景 asr_segment_ms 切段：talksage-pipeline service.rs force_segment_ms
LIMITER = 0.98
RAIL = 32000 / 32768
HOT = 28000 / 32768
METER_FULL = 0.1
METER_OVERLOAD = 0.2
MAX_SPEECH_RATIO = 0.85
TEXT_NOISE_THRESHOLD = 0.45

# scene_params() 的 asr_segment_ms。Debug 名 livetranslation 来自 finalize.rs 的 {:?}。
SCENE_FORCE_MS = {
    "dictation": 5_000,
    "conversation": 4_000,
    "bilingual": 4_000,
    "translation": 4_000,
    "livetranslation": 3_000,
    "live_translation": 3_000,
    "meeting": 6_000,
    "lecture": 6_000,
    "custom": 4_000,
}
# EngineKind::is_streaming
STREAMING_ENGINES = {"paraformer-zh", "zipformer-en", "aliyun", "aliyun-cloud"}

LATIN_WORD = re.compile(r"[A-Za-z]{3,}")
CJK = re.compile(r"[\u4e00-\u9fff]")

ROOT = Path(__file__).resolve().parents[1]


@dataclass
class WavStats:
    path: str
    sample_rate: int
    channels: int
    duration_s: float
    peak: float
    rms: float
    rail_ratio: float
    hot_ratio: float
    seconds: int
    full_seconds: int
    overload_seconds: int
    max_second_rms: float

    def to_dict(self) -> dict:
        data = asdict(self)
        data["full_ratio"] = self.full_ratio
        data["overload_ratio"] = self.overload_ratio
        data["crest_db"] = round(self.crest_db, 2)
        return data

    @property
    def full_ratio(self) -> float:
        return self.full_seconds / self.seconds if self.seconds else 0.0

    @property
    def overload_ratio(self) -> float:
        return self.overload_seconds / self.seconds if self.seconds else 0.0

    @property
    def crest_db(self) -> float:
        if self.rms <= 1e-9 or self.peak <= 0:
            return 0.0
        return 20.0 * math.log10(self.peak / self.rms)


def scan_wav(path: Path) -> WavStats:
    """按秒扫描 PCM16。立体声取左声道（主录音左声道是麦克风）。"""
    with wave.open(str(path), "rb") as wav:
        channels = wav.getnchannels()
        rate = wav.getframerate()
        width = wav.getsampwidth()
        if width != 2:
            raise RuntimeError(f"只支持 PCM16，实际 sample_width={width}: {path}")
        if channels < 1 or rate <= 0:
            raise RuntimeError(f"WAV 参数无效: channels={channels} rate={rate}")
        sumsq = 0.0
        count = 0
        peak = 0.0
        rail = 0
        hot = 0
        seconds = 0
        full_seconds = 0
        overload_seconds = 0
        max_second_rms = 0.0
        while True:
            raw = wav.readframes(rate)
            if not raw:
                break
            values = struct.unpack(f"<{len(raw) // 2}h", raw)
            mono = values[0::channels]
            if not mono:
                break
            sec_sumsq = 0.0
            for sample in mono:
                amplitude = abs(sample / 32768.0)
                peak = max(peak, amplitude)
                sec_sumsq += amplitude * amplitude
                if amplitude >= RAIL:
                    rail += 1
                if amplitude >= HOT:
                    hot += 1
            n = len(mono)
            sumsq += sec_sumsq
            count += n
            sec_rms = math.sqrt(sec_sumsq / n)
            seconds += 1
            if sec_rms >= METER_FULL:
                full_seconds += 1
            if sec_rms >= METER_OVERLOAD:
                overload_seconds += 1
            max_second_rms = max(max_second_rms, sec_rms)
    rms = math.sqrt(sumsq / count) if count else 0.0
    duration = count / rate if rate else 0.0
    return WavStats(
        path=str(path),
        sample_rate=rate,
        channels=channels,
        duration_s=round(duration, 3),
        peak=round(peak, 4),
        rms=round(rms, 4),
        rail_ratio=round(rail / count, 6) if count else 0.0,
        hot_ratio=round(hot / count, 6) if count else 0.0,
        seconds=seconds,
        full_seconds=full_seconds,
        overload_seconds=overload_seconds,
        max_second_rms=round(max_second_rms, 4),
    )


def force_segment_ms(runtime: dict | None) -> int | None:
    if not runtime:
        return None
    engine = str(runtime.get("user_engine") or "")
    if engine in STREAMING_ENGINES:
        return None
    scene = str(runtime.get("scene_mode") or "").lower()
    return SCENE_FORCE_MS.get(scene)


def summarize_segments(rows: list[dict], force_ms: int | None) -> dict:
    speakers: Counter[str] = Counter()
    durations = []
    rms_values = []
    chars = 0
    at_cap = 0
    overlaps = 0
    latin_examples: list[str] = []
    very_short = 0
    duplicate_run = 0
    previous = None
    for index, row in enumerate(rows):
        text = str(row.get("text") or "")
        label = str(row.get("speaker_label") or "")
        speakers[label] += 1
        chars += len(text.strip())
        duration = int(row.get("duration_ms") or 0)
        durations.append(duration)
        rms_values.append(float(row.get("rms") or 0.0))
        if force_ms and force_ms - 80 <= duration <= force_ms + 50:
            at_cap += 1
        if len(text.strip()) <= 2:
            very_short += 1
        if previous is not None and text and text == previous:
            duplicate_run += 1
        previous = text
        if CJK.search(text) and LATIN_WORD.search(text) and len(latin_examples) < 5:
            latin_examples.append(_clip(text, 72))
        if index + 1 < len(rows):
            nxt = rows[index + 1]
            end = int(row.get("ts_ms") or 0) + duration
            if int(nxt.get("ts_ms") or 0) < end - 400:
                overlaps += 1
    return {
        "count": len(rows),
        "chars": chars,
        "very_short": very_short,
        "duplicate_runs": duplicate_run,
        "at_force_cap": at_cap,
        "force_ms": force_ms,
        "overlaps": overlaps,
        "duration_max": max(durations) if durations else 0,
        "rms_p50": _percentile(rms_values, 0.5),
        "rms_p90": _percentile(rms_values, 0.9),
        "latin_segments": _count_latin(rows),
        "latin_examples": latin_examples,
        "speakers": speakers.most_common(),
    }


def _count_latin(rows: list[dict]) -> int:
    total = 0
    for row in rows:
        text = str(row.get("text") or "")
        if CJK.search(text) and LATIN_WORD.search(text):
            total += 1
    return total


def _percentile(values: list[float], q: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int(round((len(ordered) - 1) * q))))
    return round(ordered[index], 4)


def _clip(text: str, limit: int) -> str:
    compact = " ".join(text.split())
    if len(compact) <= limit:
        return compact
    return compact[: limit - 1] + "…"


def _snr_db(speech: float, noise: float) -> float | None:
    if speech <= 0 or noise <= 0:
        return None
    return 20.0 * math.log10(speech / noise)


def build_findings(meta: dict, runtime: dict | None, wav: WavStats | None, segments: dict) -> list[dict]:
    runtime = runtime or {}
    findings = []
    gain = float(runtime.get("input_gain_db") or 0.0)
    denoise = bool(runtime.get("denoise_enabled"))
    engine = str(runtime.get("user_engine") or "未知引擎")
    scene = str(runtime.get("scene_mode") or "未知场景")

    if wav and wav.peak >= 0.97 and wav.rail_ratio >= 0.005:
        gain_text = f"采集先加了 {gain:.0f} dB，再钳在 ±{LIMITER:.2f}。" if gain >= 6 else f"采样被钳在 ±{LIMITER:.2f}。"
        findings.append({
            "code": "hard_clip",
            "summary": (
                f"{gain_text}峰值 {wav.peak:.3f}，{wav.rail_ratio:.2%} 的采样贴在限位上。"
                "电平表顶满时，辅音已经被削平。"
            ),
            "evidence": {"peak": wav.peak, "rail_ratio": wav.rail_ratio, "input_gain_db": gain},
        })

    full_ratio = wav.full_ratio if wav else 0.0
    overload_ratio = wav.overload_ratio if wav else 0.0
    avg_rms = float(meta.get("avg_rms") or 0.0)
    if overload_ratio >= 0.25 or avg_rms >= METER_OVERLOAD:
        where = ""
        if wav and wav.seconds:
            where = f"录音里 {full_ratio:.0%} 的秒是 100%，{overload_ratio:.0%} 的秒是过载。"
        gain_text = f"输入增益 {gain:.0f} dB。" if gain >= 6 else ""
        findings.append({
            "code": "meter_overload",
            "summary": (
                f"电平表在 RMS≥{METER_FULL} 时已经是 100%，RMS≥{METER_OVERLOAD} 标成过载。"
                f"这场平均 RMS {avg_rms:.3f}。{where}{gain_text}"
            ),
            "evidence": {
                "avg_rms": avg_rms,
                "full_ratio": round(full_ratio, 4),
                "overload_ratio": round(overload_ratio, 4),
                "input_gain_db": gain,
            },
        })

    noise = _stream_noise(meta)
    snr = _snr_db(avg_rms, noise)
    if snr is not None and snr < 6.0 and noise >= 0.02:
        denoise_text = "降噪开着。" if denoise else "降噪关着。"
        findings.append({
            "code": "low_snr",
            "summary": (
                f"非语音 RMS {noise:.3f}，语音 RMS {avg_rms:.3f}，大约 {snr:.1f} dB。{denoise_text}"
                "房间底噪和人声贴在一起，VAD 会把大段环境声当成语音。"
            ),
            "evidence": {"avg_rms": avg_rms, "non_speech_avg_rms": noise, "snr_db": round(snr, 2)},
        })

    ratio = float(meta.get("speech_ratio") or 0.0)
    if ratio > MAX_SPEECH_RATIO:
        skipped = "会后分析被跳过。" if meta.get("skipped_analysis") else ""
        findings.append({
            "code": "continuous_speech",
            "summary": (
                f"语音占比 {ratio:.1%}，超过 {MAX_SPEECH_RATIO:.2f}，质量按「几乎无停顿」标成噪音。{skipped}"
            ),
            "evidence": {"speech_ratio": ratio, "quality": meta.get("quality"), "skipped_analysis": bool(meta.get("skipped_analysis"))},
        })

    force_ms = segments.get("force_ms")
    count = int(segments.get("count") or 0)
    at_cap = int(segments.get("at_force_cap") or 0)
    if force_ms and count >= 8 and at_cap / count >= 0.2:
        findings.append({
            "code": "force_cut",
            "summary": (
                f"{engine} 不是流式引擎。{scene} 场景每 {force_ms} ms 强制切段，"
                f"{at_cap}/{count} 段顶在这个时长上，切点落在句子中间。"
            ),
            "evidence": {"force_ms": force_ms, "at_cap": at_cap, "count": count, "overlaps": segments.get("overlaps")},
        })

    latin_segments = int(segments.get("latin_segments") or 0)
    if count and latin_segments >= 3 and latin_segments / count >= 0.05:
        example = ""
        examples = segments.get("latin_examples") or []
        if examples:
            example = f"例如「{examples[0]}」。"
        findings.append({
            "code": "latin_mix",
            "summary": (
                f"{latin_segments}/{count} 段中文里夹了拉丁字母词。{example}"
                "削波和低信噪比时，多语段级模型会这样补词。"
            ),
            "evidence": {"latin_segments": latin_segments, "examples": examples},
        })

    for stream in meta.get("streams") or []:
        finals = int(stream.get("final_segments") or 0)
        total_ms = int(stream.get("total_ms") or 0)
        label = str(stream.get("speaker_label") or "")
        if runtime.get("client_enabled") and label and label != "我" and finals == 0 and total_ms >= 1000:
            findings.append({
                "code": "idle_peer",
                "summary": (
                    f"「{label}」录了 {total_ms / 1000:.1f} 秒、0 段转写。"
                    "回环没有持续采到人声，会议室里的声音都进了麦克风这一路。"
                ),
                "evidence": {"speaker_label": label, "total_ms": total_ms, "final_segments": 0},
            })
            break

    speakers = segments.get("speakers") or []
    if str(runtime.get("speaker_mode") or "") == "voiceprint" and count >= 15 and speakers:
        top_label, top_count = speakers[0]
        share = top_count / count
        if share >= 0.75:
            listed = ", ".join(f"{name}={n}" for name, n in speakers[:4])
            findings.append({
                "code": "speaker_collapse",
                "summary": (
                    f"声纹把 {count} 段收成 {len(speakers)} 个标签，{top_label} 占 {share:.0%}（{listed}）。"
                ),
                "evidence": {"speakers": speakers[:6], "top_share": round(share, 4)},
            })
    return findings


def _stream_noise(meta: dict) -> float:
    streams = meta.get("streams") or []
    mine = next((s for s in streams if s.get("speaker_label") == "我"), None)
    if mine is None and streams:
        mine = max(streams, key=lambda s: int(s.get("total_ms") or 0))
    if not mine:
        return 0.0
    return float(mine.get("non_speech_avg_rms") or 0.0)


def quality_notes(meta: dict) -> list[str]:
    """用默认质量规则解释 sessions.meta.quality，不重新改判。"""
    notes = []
    ratio = float(meta.get("speech_ratio") or 0.0)
    avg = float(meta.get("avg_rms") or 0.0)
    text_noise = float(meta.get("text_noise") or 0.0)
    noise = _stream_noise(meta)
    high_rms = max(noise * 5.0, 0.2)
    if ratio > MAX_SPEECH_RATIO:
        notes.append(f"语音占比 {ratio:.1%} > {MAX_SPEECH_RATIO:.2f}，几乎无停顿")
    if avg > high_rms:
        notes.append(f"平均 RMS {avg:.3f} > 自动高能量阈值 {high_rms:.3f}")
    if text_noise > TEXT_NOISE_THRESHOLD:
        notes.append(f"文本噪音 {text_noise:.3f} > {TEXT_NOISE_THRESHOLD:.2f}")
    if not notes and meta.get("quality") == "noise":
        notes.append("质量是噪音，但没有命中上面三条默认规则，需看当时的配置覆盖")
    return notes


def find_wav(data_dir: Path, session_id: int, meta: dict) -> Path | None:
    streams = list(meta.get("streams") or [])
    streams.sort(key=lambda s: (s.get("speaker_label") != "我", -int(s.get("total_ms") or 0)))
    for stream in streams:
        recording = stream.get("recording")
        if not recording:
            continue
        path = Path(str(recording))
        if path.is_file():
            return path
        rooted = data_dir / path
        if rooted.is_file():
            return rooted
    rec_dir = data_dir / "sessions" / str(session_id) / "recordings"
    if not rec_dir.is_dir():
        return None
    named = sorted(rec_dir.glob("*_我.wav"))
    if named:
        return named[-1]
    any_wav = sorted(rec_dir.glob("*.wav"))
    return any_wav[-1] if any_wav else None


def connect(db_path: Path) -> sqlite3.Connection:
    # 只读打开，诊断不能改会话库。as_uri() 在 Windows 上给出 file:///D:/...
    conn = sqlite3.connect(f"{db_path.resolve().as_uri()}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    return conn


def list_sessions(conn: sqlite3.Connection, limit: int) -> list[dict]:
    columns = {row[1] for row in conn.execute("PRAGMA table_info(sessions)")}
    title = "title" if "title" in columns else "NULL AS title"
    rows = conn.execute(
        f"SELECT id, {title}, started_at, ended_at FROM sessions ORDER BY id DESC LIMIT ?",
        (limit,),
    ).fetchall()
    return [dict(row) for row in rows]


def resolve_session(conn: sqlite3.Connection, *, session_id: int | None, title: str | None, from_end: int | None) -> dict:
    columns = {row[1] for row in conn.execute("PRAGMA table_info(sessions)")}
    if "title" not in columns and title:
        raise SystemExit("这个 sessions.db 没有 title 列，请改用 --id")
    if session_id is not None:
        row = conn.execute("SELECT * FROM sessions WHERE id = ?", (session_id,)).fetchone()
        if row is None:
            raise SystemExit(f"没有会话 {session_id}")
        return dict(row)
    if title:
        exact = conn.execute("SELECT * FROM sessions WHERE title = ? ORDER BY id DESC", (title,)).fetchall()
        rows = exact or conn.execute(
            "SELECT * FROM sessions WHERE title LIKE ? ORDER BY id DESC",
            (f"%{title}%",),
        ).fetchall()
        if not rows:
            raise SystemExit(f"没有标题匹配 {title!r} 的会话")
        if len(rows) > 1 and not exact:
            listing = ", ".join(f"{row['id']}:{row['title']}" for row in rows[:8])
            raise SystemExit(f"标题匹配到多条，请用 --id 指定：{listing}")
        return dict(rows[0])
    if from_end is not None:
        if from_end < 1:
            raise SystemExit("--from-end 从 1 开始，1 是最新一条")
        rows = conn.execute("SELECT * FROM sessions ORDER BY id DESC LIMIT ?", (from_end,)).fetchall()
        if len(rows) < from_end:
            raise SystemExit(f"只有 {len(rows)} 条会话，没有倒数第 {from_end} 条")
        return dict(rows[-1])
    raise SystemExit("请指定 --title、--id 或 --from-end")


def load_segments(conn: sqlite3.Connection, session_id: int) -> list[dict]:
    columns = {row[1] for row in conn.execute("PRAGMA table_info(segments)")}
    duration = "duration_ms" if "duration_ms" in columns else "0 AS duration_ms"
    rms = "rms" if "rms" in columns else "0 AS rms"
    rows = conn.execute(
        f"""SELECT speaker_label, text, ts_ms, {duration}, {rms}
            FROM segments WHERE session_id = ? ORDER BY ts_ms, id""",
        (session_id,),
    ).fetchall()
    return [dict(row) for row in rows]


def diagnose(data_dir: Path, *, session_id: int | None, title: str | None, from_end: int | None) -> dict:
    db_path = data_dir / "sessions.db"
    if not db_path.is_file():
        raise SystemExit(f"找不到 {db_path}")
    conn = connect(db_path)
    try:
        row = resolve_session(conn, session_id=session_id, title=title, from_end=from_end)
        segments = load_segments(conn, int(row["id"]))
    finally:
        conn.close()
    meta = json.loads(row["meta"]) if row.get("meta") else {}
    runtime = meta.get("runtime_info") or {}
    wav_path = find_wav(data_dir, int(row["id"]), meta)
    wav = scan_wav(wav_path) if wav_path else None
    segment_summary = summarize_segments(segments, force_segment_ms(runtime))
    findings = build_findings(meta, runtime, wav, segment_summary)
    return {
        "session": {
            "id": row["id"],
            "title": row.get("title"),
            "started_at": row.get("started_at"),
            "ended_at": row.get("ended_at"),
        },
        "meta": {
            "quality": meta.get("quality"),
            "skipped_analysis": meta.get("skipped_analysis"),
            "duration_ms": meta.get("duration_ms"),
            "speech_ms": meta.get("speech_ms"),
            "speech_ratio": meta.get("speech_ratio"),
            "avg_rms": meta.get("avg_rms"),
            "max_rms": meta.get("max_rms"),
            "text_noise": meta.get("text_noise"),
            "master_recording": meta.get("master_recording"),
        },
        "runtime": runtime,
        "streams": meta.get("streams") or [],
        "quality_notes": quality_notes(meta),
        "wav": wav.to_dict() if wav else None,
        "segments": segment_summary,
        "findings": findings,
    }


def render_text(report: dict) -> str:
    session = report["session"]
    meta = report["meta"]
    runtime = report["runtime"] or {}
    title = session.get("title") or "(无标题)"
    lines = [
        f"会话 {session['id']}  {title}",
        (
            f"场景 {runtime.get('scene_mode') or '-'} · 引擎 {runtime.get('user_engine') or '-'} · "
            f"增益 {float(runtime.get('input_gain_db') or 0):.1f} dB · "
            f"自动增益 {'开' if runtime.get('auto_gain') else '关'} · "
            f"降噪 {'开' if runtime.get('denoise_enabled') else '关'} · "
            f"说话人 {runtime.get('speaker_mode') or '-'}"
        ),
        (
            f"质量 {meta.get('quality') or '-'} · 语音占比 {float(meta.get('speech_ratio') or 0):.1%} · "
            f"平均 RMS {float(meta.get('avg_rms') or 0):.3f} · "
            f"会后分析 {'跳过' if meta.get('skipped_analysis') else '保留'}"
        ),
    ]
    if report["quality_notes"]:
        lines.append("质量规则：" + "；".join(report["quality_notes"]))
    wav = report.get("wav")
    if wav:
        lines.append(
            f"录音 {wav['path']}"
        )
        lines.append(
            f"峰值 {wav['peak']:.3f} · 贴限幅 {wav['rail_ratio']:.2%} · 文件 RMS {wav['rms']:.3f} · "
            f"100% 秒占比 {wav['full_ratio']:.0%} · 过载秒占比 {wav['overload_ratio']:.0%}"
        )
    else:
        lines.append("没有找到分轨 WAV，只根据会话库判断。")
    seg = report["segments"]
    speaker_text = ", ".join(f"{name}={count}" for name, count in seg["speakers"][:6]) or "无"
    cap = ""
    if seg.get("force_ms"):
        cap = f" · 顶满 {seg['force_ms']} ms 切段 {seg['at_force_cap']} 段"
    lines.append(f"转写 {seg['count']} 段 · {seg['chars']} 字 · 说话人 {speaker_text}{cap}")
    if report["findings"]:
        lines.append("")
        lines.append("结论")
        for index, finding in enumerate(report["findings"], start=1):
            lines.append(f"{index}. {finding['summary']}")
    else:
        lines.append("")
        lines.append("结论：没有命中削波、电平过载、低信噪比或强制切段堆积。")
    return "\n".join(lines)


def default_data_dir() -> Path:
    env = os.environ.get("TALKSAGE_DATA_DIR")
    if env:
        return Path(env)
    project = ROOT / "data"
    if (project / "sessions.db").is_file():
        return project
    return Path.home() / ".talksage"


def main(argv: list[str] | None = None) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description="诊断历史会话的电平、削波和转写质量")
    parser.add_argument("--data-dir", type=Path, default=None, help="含 sessions.db 的数据目录，默认 TALKSAGE_DATA_DIR 或项目 data/")
    parser.add_argument("--id", type=int, dest="session_id")
    parser.add_argument("--title", help="会话标题。先精确匹配（多条时取 id 最大），没有再按包含匹配")
    parser.add_argument("--from-end", type=int, help="倒数第 N 条，1 是最新")
    parser.add_argument("--list", action="store_true", help="只列出最近会话")
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    data_dir = args.data_dir or default_data_dir()
    db_path = data_dir / "sessions.db"
    if not db_path.is_file():
        print(f"找不到 {db_path}", file=sys.stderr)
        return 1
    if args.list or (args.session_id is None and not args.title and args.from_end is None):
        conn = connect(db_path)
        try:
            rows = list_sessions(conn, args.limit)
        finally:
            conn.close()
        if args.json:
            print(json.dumps(rows, ensure_ascii=False, indent=2))
        else:
            if not rows:
                print(f"{db_path} 里没有会话")
                return 0
            for row in rows:
                print(f"{row['id']:>6}  {row.get('title') or '(无标题)'}")
        return 0
    report = diagnose(data_dir, session_id=args.session_id, title=args.title, from_end=args.from_end)
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print(render_text(report))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit as exc:
        if isinstance(exc.code, str):
            print(exc.code, file=sys.stderr)
            raise SystemExit(1)
        raise
