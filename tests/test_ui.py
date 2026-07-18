import pytest
from PySide6.QtWidgets import QApplication
from ui.sections.terms import TermsSection
from ui.sections.transcript import TranscriptSection
from ui.main_window import MainWindow
from core.models import PluginResult, TranscriptSegment


@pytest.fixture(scope="session")
def qapp():
    return QApplication.instance() or QApplication([])


def test_main_window_shows_asr_status(qapp, qtbot):
    window = MainWindow()
    qtbot.addWidget(window)
    window.asr_status_received.emit("ASR 加载中…")
    assert "加载" in window._status_badge.text()
    window.asr_status_received.emit("ASR 就绪")
    assert "就绪" in window._status_badge.text()


def test_terms_section_updates_skeleton_by_result_id(qapp, qtbot):
    section = TermsSection()
    qtbot.addWidget(section)
    skeleton = PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = …",
        result_id="term-1",
        status="skeleton",
    )
    final = PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = 新产品导入",
        result_id="term-1",
        status="final",
    )
    section.add_result(skeleton)
    assert section.count() == 1
    section.add_result(final)
    assert section.count() == 1
    assert "新产品导入" in section.item(0).text()


def test_terms_section_adds_result(qapp, qtbot):
    section = TermsSection()
    qtbot.addWidget(section)
    result = PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="NPI = 新产品导入流程",
    )
    section.add_result(result)
    assert section.count() == 1


def test_terms_section_shows_content(qapp, qtbot):
    section = TermsSection()
    qtbot.addWidget(section)
    result = PluginResult(
        plugin_name="term_explainer",
        ui_section="terms",
        content="BOQ = 物料清单",
    )
    section.add_result(result)
    item_text = section.item(0).text()
    assert "BOQ" in item_text


def test_transcript_section_adds_segment(qapp, qtbot):
    section = TranscriptSection()
    qtbot.addWidget(section)
    seg = TranscriptSegment(speaker="client", text="hello world", language="en")
    section.add_segment(seg)
    assert section.document().toPlainText() != ""


def test_transcript_section_shows_speaker(qapp, qtbot):
    section = TranscriptSection()
    qtbot.addWidget(section)
    seg = TranscriptSegment(speaker="client", text="our NPI schedule", language="en")
    section.add_segment(seg)
    text = section.document().toPlainText()
    assert "客户" in text  # client speaker is shown as "客户" in the redesigned UI
