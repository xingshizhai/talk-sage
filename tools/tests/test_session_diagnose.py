import importlib.util
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
import wave


SCRIPT = Path(__file__).resolve().parents[1] / "session_diagnose.py"
SPEC = importlib.util.spec_from_file_location("talksage_session_diagnose", SCRIPT)
diag = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = diag
SPEC.loader.exec_module(diag)


def _tone(path: Path, samples: list[int], channels: int = 1, rate: int = 16000) -> None:
    with wave.open(str(path), "wb") as output:
        output.setnchannels(channels)
        output.setsampwidth(2)
        output.setframerate(rate)
        frames = []
        if channels == 1:
            frames = samples
        else:
            for sample in samples:
                frames.extend((sample, 0))
        output.writeframes(b"".join(int(value).to_bytes(2, "little", signed=True) for value in frames))


class ScanWavTests(unittest.TestCase):
    def test_constant_rail_is_clipped_and_over_meter(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "clip.wav"
            _tone(path, [32111] * 16000)
            stats = diag.scan_wav(path)
            self.assertAlmostEqual(stats.peak, 32111 / 32768, places=3)
            self.assertEqual(stats.rail_ratio, 1.0)
            self.assertEqual(stats.overload_seconds, 1)
            self.assertEqual(stats.full_seconds, 1)
            self.assertGreaterEqual(stats.peak, 0.97)

    def test_stereo_uses_left_channel(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "stereo.wav"
            _tone(path, [32111] * 1600, channels=2, rate=1600)
            stats = diag.scan_wav(path)
            self.assertEqual(stats.channels, 2)
            self.assertEqual(stats.rail_ratio, 1.0)
            self.assertAlmostEqual(stats.duration_s, 1.0, places=2)


class FindingTests(unittest.TestCase):
    def test_hot_meeting_reports_clip_meter_snr_and_cut(self):
        meta = {
            "quality": "noise",
            "skipped_analysis": True,
            "speech_ratio": 0.872,
            "avg_rms": 0.235,
            "text_noise": 0.045,
            "streams": [
                {"speaker_label": "我", "total_ms": 1_113_000, "final_segments": 227, "avg_rms": 0.235, "non_speech_avg_rms": 0.181},
                {"speaker_label": "对方", "total_ms": 5100, "final_segments": 0, "avg_rms": 0.191, "non_speech_avg_rms": 0.191},
            ],
        }
        runtime = {
            "scene_mode": "meeting",
            "user_engine": "whisper-large-v3-turbo-metal",
            "input_gain_db": 12.0,
            "denoise_enabled": False,
            "client_enabled": True,
            "speaker_mode": "voiceprint",
        }
        wav = diag.WavStats(
            path="mic.wav", sample_rate=16000, channels=1, duration_s=1113.3,
            peak=0.9799, rms=0.2346, rail_ratio=0.0152, hot_ratio=0.0217,
            seconds=1114, full_seconds=689, overload_seconds=473, max_second_rms=0.6626,
        )
        segments = diag.summarize_segments(
            [
                {"speaker_label": "讲话者1", "text": "数据不管是咱们现在是metris dream", "ts_ms": 0, "duration_ms": 6000, "rms": 0.3},
                {"speaker_label": "讲话者1", "text": "是产后呢industry没有", "ts_ms": 1200, "duration_ms": 6000, "rms": 0.3},
                {"speaker_label": "讲话者1", "text": "改后没有DTS但是offsetting pass", "ts_ms": 2400, "duration_ms": 6000, "rms": 0.3},
                *[{"speaker_label": "讲话者1", "text": "继续讨论预算", "ts_ms": 8000 + i * 6000, "duration_ms": 6000, "rms": 0.22} for i in range(12)],
                *[{"speaker_label": "讲话者", "text": "嗯", "ts_ms": 90000 + i * 1000, "duration_ms": 800, "rms": 0.05} for i in range(3)],
            ],
            diag.force_segment_ms(runtime),
        )
        codes = [item["code"] for item in diag.build_findings(meta, runtime, wav, segments)]
        self.assertEqual(
            codes,
            ["hard_clip", "meter_overload", "low_snr", "continuous_speech", "force_cut", "latin_mix", "idle_peer", "speaker_collapse"],
        )
        notes = diag.quality_notes(meta)
        self.assertTrue(any("几乎无停顿" in note for note in notes))
        self.assertFalse(any("高能量" in note for note in notes))

    def test_quiet_clean_session_has_no_findings(self):
        meta = {
            "quality": "clean",
            "skipped_analysis": False,
            "speech_ratio": 0.5,
            "avg_rms": 0.04,
            "text_noise": 0.02,
            "streams": [
                {"speaker_label": "我", "total_ms": 60_000, "final_segments": 8, "non_speech_avg_rms": 0.005},
                {"speaker_label": "对方", "total_ms": 40_000, "final_segments": 6, "non_speech_avg_rms": 0.004},
            ],
        }
        runtime = {
            "scene_mode": "conversation",
            "user_engine": "qwen3-asr",
            "input_gain_db": 0,
            "denoise_enabled": False,
            "client_enabled": True,
            "speaker_mode": "channel",
        }
        wav = diag.WavStats(
            path="quiet.wav", sample_rate=16000, channels=1, duration_s=60,
            peak=0.3, rms=0.04, rail_ratio=0.0, hot_ratio=0.0,
            seconds=60, full_seconds=2, overload_seconds=0, max_second_rms=0.12,
        )
        segments = diag.summarize_segments(
            [{"speaker_label": "我", "text": "今天把接口定下来", "ts_ms": i * 4000, "duration_ms": 2000, "rms": 0.04} for i in range(8)],
            diag.force_segment_ms(runtime),
        )
        self.assertEqual(diag.build_findings(meta, runtime, wav, segments), [])

    def test_streaming_engine_does_not_force_cut(self):
        self.assertIsNone(diag.force_segment_ms({"user_engine": "paraformer-zh", "scene_mode": "meeting"}))
        self.assertEqual(diag.force_segment_ms({"user_engine": "qwen3-asr", "scene_mode": "meeting"}), 6000)


class SessionLookupTests(unittest.TestCase):
    def test_title_and_from_end_select_the_recorded_session(self):
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory)
            db = data / "sessions.db"
            wav = data / "mic.wav"
            _tone(wav, [1000] * 1600, rate=1600)
            meta = {
                "quality": "clean",
                "skipped_analysis": False,
                "speech_ratio": 0.4,
                "avg_rms": 0.03,
                "text_noise": 0.0,
                "streams": [{
                    "speaker_label": "我",
                    "total_ms": 1000,
                    "speech_ms": 400,
                    "final_segments": 1,
                    "avg_rms": 0.03,
                    "non_speech_avg_rms": 0.004,
                    "recording": str(wav),
                }],
                "runtime_info": {"scene_mode": "dictation", "user_engine": "qwen3-asr", "input_gain_db": 0, "denoise_enabled": False, "speaker_mode": "off"},
            }
            conn = sqlite3.connect(db)
            conn.execute("CREATE TABLE sessions (id INTEGER PRIMARY KEY, started_at INTEGER, ended_at INTEGER, title TEXT, meta TEXT)")
            conn.execute(
                "CREATE TABLE segments (id INTEGER PRIMARY KEY, session_id INTEGER, speaker_label TEXT, text TEXT, ts_ms INTEGER, duration_ms INTEGER, rms REAL)"
            )
            conn.execute("INSERT INTO sessions (id, started_at, ended_at, title, meta) VALUES (1, 1, 2, '其他', NULL)")
            conn.execute(
                "INSERT INTO sessions (id, started_at, ended_at, title, meta) VALUES (2, 3, 4, ?, ?)",
                ("JMH-1015", json.dumps(meta)),
            )
            conn.execute(
                "INSERT INTO segments (session_id, speaker_label, text, ts_ms, duration_ms, rms) VALUES (2, '我', '你好', 0, 1000, 0.03)"
            )
            conn.commit()
            conn.close()

            by_title = diag.diagnose(data, session_id=None, title="JMH-1015", from_end=None)
            by_end = diag.diagnose(data, session_id=None, title=None, from_end=1)
            self.assertEqual(by_title["session"]["id"], 2)
            self.assertEqual(by_end["session"]["title"], "JMH-1015")
            self.assertEqual(by_title["wav"]["sample_rate"], 1600)
            self.assertEqual(by_title["findings"], [])
            text = diag.render_text(by_title)
            self.assertIn("会话 2  JMH-1015", text)


if __name__ == "__main__":
    unittest.main()
