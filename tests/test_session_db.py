from core.session_db import SessionDatabase
from core.models import TranscriptSegment, PluginResult


def test_session_db_roundtrip(tmp_path):
    db = SessionDatabase(tmp_path / "talksage.db")
    sid = db.start_session(stamp="20260718-120000", markdown_path=str(tmp_path / "a.md"))
    db.add_segment(sid, TranscriptSegment(speaker="client", text="NPI schedule", language="en"))
    db.add_term(sid, "NPI = 新产品导入")
    db.set_notes(sid, "## 纪要\n- NPI")
    db.end_session(sid)

    sessions = db.list_sessions()
    assert len(sessions) == 1
    assert sessions[0]["stamp"] == "20260718-120000"
    assert sessions[0]["segment_count"] == 1

    detail = db.get_session(sid)
    assert detail["notes"] and "NPI" in detail["notes"]
    assert any("NPI schedule" in s["text"] for s in detail["segments"])
    assert "新产品导入" in detail["terms"][0]


def test_session_db_search(tmp_path):
    db = SessionDatabase(tmp_path / "talksage.db")
    sid = db.start_session(stamp="s1", markdown_path="")
    db.add_segment(sid, TranscriptSegment(speaker="client", text="please check the BOQ", language="en"))
    db.end_session(sid)
    hits = db.search("BOQ")
    assert hits
    assert hits[0]["session_id"] == sid
