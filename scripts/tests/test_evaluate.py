import importlib.util
import math
from pathlib import Path
import tempfile
import unittest
import wave


SCRIPT = Path(__file__).resolve().parents[1] / "evaluate.py"
SPEC = importlib.util.spec_from_file_location("talksage_evaluate", SCRIPT)
evaluate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(evaluate)


class EvaluateTests(unittest.TestCase):
    def test_analyze_wav_reports_duration_level_and_format(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "tone.wav"
            rate = 16000
            with wave.open(str(path), "wb") as output:
                output.setnchannels(1)
                output.setsampwidth(2)
                output.setframerate(rate)
                samples = [int(math.sin(2 * math.pi * 440 * i / rate) * 8192) for i in range(rate)]
                output.writeframes(b"".join(value.to_bytes(2, "little", signed=True) for value in samples))
            metrics = evaluate.analyze_wav(path, 1.0)
            self.assertEqual(metrics["sample_rate"], 16000)
            self.assertEqual(metrics["channels"], 1)
            self.assertAlmostEqual(metrics["duration_seconds"], 1.0)
            self.assertGreater(metrics["rms"], 0.1)
            self.assertEqual(metrics["capture_drift_ratio"], 0.0)

    def test_score_prefers_accurate_fast_low_latency_model(self):
        report = {"asr": {"results": [
            {"engine": "fast", "status": "ok", "error_rate": 0.1, "rtf": 0.1, "first_final_ms": 300},
            {"engine": "slow", "status": "ok", "error_rate": 0.2, "rtf": 0.7, "first_final_ms": 1200},
        ]}}
        config = evaluate.load_json(evaluate.DEFAULT_CONFIG)
        evaluate.score_and_gate(report, config)
        self.assertEqual(report["recommendation"], "fast")
        self.assertGreater(report["asr"]["results"][0]["score"], report["asr"]["results"][1]["score"])

    def test_bad_candidate_does_not_fail_a_healthy_baseline(self):
        report = {"recommendation": "paraformer-zh", "asr": {"results": [
            {"engine": "paraformer-zh", "status": "ok", "gate_failures": []},
            {"engine": "candidate", "status": "ok", "gate_failures": ["error_rate"]},
        ]}}
        config = evaluate.load_json(evaluate.DEFAULT_CONFIG)
        self.assertFalse(evaluate.report_failed(report, config))

    def test_evaluation_config_includes_metal_engine(self):
        config = evaluate.load_json(evaluate.DEFAULT_CONFIG)
        self.assertIn("whisper-large-v3-turbo-metal", config["engines"])


if __name__ == "__main__":
    unittest.main()
