from PySide6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QPushButton, QGroupBox
)
from PySide6.QtCore import Signal, Slot
from core.models import TranscriptSegment, PluginResult
from ui.sections.transcript import TranscriptSection
from ui.sections.terms import TermsSection


class MainWindow(QMainWindow):
    # Signals used to safely update UI from background threads
    segment_received = Signal(object)
    result_received = Signal(object)

    def __init__(self):
        super().__init__()
        self.setWindowTitle("TalkSage")
        self.setMinimumWidth(320)
        self.setMaximumWidth(480)

        central = QWidget()
        self.setCentralWidget(central)
        layout = QVBoxLayout(central)

        # Record toggle button
        self._record_btn = QPushButton("▶ 开始监听")
        self._record_btn.setCheckable(True)
        self._record_btn.toggled.connect(self._on_record_toggled)
        layout.addWidget(self._record_btn)

        # Transcript section
        transcript_box = QGroupBox("🎙 实时转写")
        transcript_layout = QVBoxLayout(transcript_box)
        self.transcript_section = TranscriptSection()
        transcript_layout.addWidget(self.transcript_section)
        layout.addWidget(transcript_box)

        # Terms section
        terms_box = QGroupBox("📖 术语")
        terms_layout = QVBoxLayout(terms_box)
        self.terms_section = TermsSection()
        terms_layout.addWidget(self.terms_section)
        layout.addWidget(terms_box)

        layout.addStretch()

        # Connect signals to UI slots
        self.segment_received.connect(self.transcript_section.add_segment)
        self.result_received.connect(self._route_result)

    @Slot(bool)
    def _on_record_toggled(self, checked: bool) -> None:
        self._record_btn.setText("⏹ 停止监听" if checked else "▶ 开始监听")

    @Slot(object)
    def _route_result(self, result: PluginResult) -> None:
        if result.ui_section == "terms":
            self.terms_section.add_result(result)
