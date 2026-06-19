import pytest
import numpy as np
from unittest.mock import MagicMock, AsyncMock
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
    pipeline.on_segment = None
    pipeline.on_result = lambda r: results.append(r)

    import asyncio
    await pipeline._handle_audio(np.zeros(16000, dtype=np.float32), "client")
    assert len(results) == 1
    assert "NPI" in results[0].content
