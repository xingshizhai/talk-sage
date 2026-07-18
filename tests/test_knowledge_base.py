from pathlib import Path
from core.knowledge_base import KnowledgeBase


def _write_kb(folder: Path) -> None:
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "customer.md").write_text(
        "# Acme Corp\n\n## Pricing\n\nMOQ is 500 units. Lead time 6 weeks.\n\n"
        "## Contacts\n\nBuyer: Jane Doe\n",
        encoding="utf-8",
    )
    (folder / "notes.txt").write_text(
        "Previous meeting: customer cares about NPI schedule and BOQ accuracy.\n",
        encoding="utf-8",
    )


def test_index_and_search_returns_relevant_chunk(tmp_path):
    folder = tmp_path / "kb"
    _write_kb(folder)
    kb = KnowledgeBase()
    kb.index_folder(folder)
    hits = kb.search("What is the MOQ?", top_k=3)
    assert hits
    assert any("500" in h.text or "MOQ" in h.text for h in hits)


def test_search_empty_kb_returns_empty():
    kb = KnowledgeBase()
    assert kb.search("NPI") == []


def test_index_skips_missing_folder(tmp_path):
    kb = KnowledgeBase()
    kb.index_folder(tmp_path / "does-not-exist")
    assert kb.chunk_count == 0


def test_search_prefers_npi_note(tmp_path):
    folder = tmp_path / "kb"
    _write_kb(folder)
    kb = KnowledgeBase()
    kb.index_folder(folder)
    hits = kb.search("NPI schedule concerns", top_k=2)
    assert hits
    assert any("NPI" in h.text for h in hits)
