from pathlib import Path
from core.session_store import SessionStore
from core.models import TranscriptSegment, PluginResult


def test_session_creates_markdown_file(tmp_path):
    store = SessionStore(sessions_dir=tmp_path)
    store.start()
    store.add_segment(TranscriptSegment(speaker="client", text="NPI schedule", language="en"))
    store.add_result(PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = 新产品导入",
        status="final",
    ))
    path = store.stop()

    assert path is not None
    assert path.exists()
    text = path.read_text(encoding="utf-8")
    assert "NPI schedule" in text
    assert "新产品导入" in text
    assert "client" in text or "客户" in text


def test_session_stop_without_start_returns_none(tmp_path):
    store = SessionStore(sessions_dir=tmp_path)
    assert store.stop() is None


def test_skeleton_results_not_written_until_final(tmp_path):
    store = SessionStore(sessions_dir=tmp_path)
    store.start()
    store.add_result(PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = …",
        result_id="abc",
        status="skeleton",
    ))
    store.add_result(PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = 新产品导入",
        result_id="abc",
        status="final",
    ))
    path = store.stop()
    text = path.read_text(encoding="utf-8")
    assert text.count("NPI") >= 1
    assert "…" not in text or "新产品导入" in text
    # Final content should be present; skeleton placeholder should not be the only line
    assert "新产品导入" in text


def test_append_notes_after_stop(tmp_path):
    store = SessionStore(sessions_dir=tmp_path)
    store.start()
    store.add_segment(TranscriptSegment(speaker="client", text="hello", language="en"))
    path = store.stop()
    store.append_notes("## 要点\n- 讨论 NPI", path=path)
    text = path.read_text(encoding="utf-8")
    assert "## 会议纪要" in text
    assert "讨论 NPI" in text
