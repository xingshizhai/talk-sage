import pytest
import numpy as np
from unittest.mock import MagicMock
from core.pipeline import Pipeline
from core.models import PluginResult


@pytest.mark.asyncio
async def test_pipeline_processes_audio_segment():
    results = []

    pipeline = Pipeline.__new__(Pipeline)
    pipeline._bus = MagicMock()

    async def fake_process(seg):
        result = PluginResult(plugin_name="term_explainer", ui_section="terms", content="NPI = 新产品导入")
        yield result

    pipeline._bus.process = fake_process

    mock_engine = MagicMock()
    from core.models import TranscriptSegment
    mock_engine.transcribe.return_value = TranscriptSegment(
        speaker="client", text="NPI schedule", language="en"
    )
    pipeline._engine = mock_engine
    pipeline._echo = MagicMock()
    pipeline._echo.should_drop.return_value = False
    pipeline._sessions = None
    pipeline.on_segment = None
    pipeline.on_result = lambda r: results.append(r)
    pipeline.on_state = None

    await pipeline._handle_audio(np.zeros(16000, dtype=np.float32), "client")
    assert len(results) == 1
    assert "NPI" in results[0].content


def test_pipeline_warmup_calls_engine_and_reports_status():
    statuses = []
    pipeline = Pipeline.__new__(Pipeline)
    pipeline._engine = MagicMock()
    pipeline.on_asr_status = lambda msg: statuses.append(msg)

    pipeline.warmup()

    pipeline._engine.warmup.assert_called_once()
    assert any("加载" in s for s in statuses)
    assert any("就绪" in s for s in statuses)


def test_pipeline_warmup_reports_error_on_failure():
    statuses = []
    pipeline = Pipeline.__new__(Pipeline)
    pipeline._engine = MagicMock()
    pipeline._engine.warmup.side_effect = RuntimeError("model missing")
    pipeline.on_asr_status = lambda msg: statuses.append(msg)

    pipeline.warmup()

    assert any("失败" in s for s in statuses)


@pytest.mark.asyncio
async def test_pipeline_drops_crosstalk_segments():
    from core.echo_filter import CrosstalkFilter
    from core.models import TranscriptSegment

    pipeline = Pipeline.__new__(Pipeline)
    pipeline._bus = MagicMock()
    pipeline._sessions = None
    pipeline._echo = CrosstalkFilter(similarity_threshold=0.5, window_seconds=10)
    pipeline._echo.observe(
        TranscriptSegment(speaker="client", text="hello world from client", language="en")
    )
    pipeline._engine = MagicMock()
    pipeline._engine.transcribe.return_value = TranscriptSegment(
        speaker="user", text="hello world from client", language="zh"
    )
    called = []
    pipeline.on_segment = lambda s: called.append(s)
    pipeline.on_result = None

    await pipeline._handle_audio(np.zeros(16000, dtype=np.float32), "user")
    assert called == []
