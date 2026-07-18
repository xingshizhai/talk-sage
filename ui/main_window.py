from PySide6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QPushButton, QLabel
)
from PySide6.QtCore import Qt, Signal, Slot, QTimer
from PySide6.QtGui import QColor
from core.models import PluginResult
from ui.sections.transcript import TranscriptSection
from ui.sections.terms import TermsSection
from ui.sections.context import ContextSection
from ui.sections.suggestions import SuggestionsSection
from ui.style import STYLESHEET


class _PulseDot(QWidget):
    """Animated pulsing dot shown when recording."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedSize(12, 12)
        self._opacity = 1.0
        self._growing = False
        self._timer = QTimer(self)
        self._timer.timeout.connect(self._tick)
        self._active = False

    def set_active(self, active: bool) -> None:
        self._active = active
        if active:
            self._timer.start(40)
        else:
            self._timer.stop()
            self._opacity = 1.0
        self.update()

    def _tick(self) -> None:
        step = 0.04
        if self._growing:
            self._opacity = min(1.0, self._opacity + step)
            if self._opacity >= 1.0:
                self._growing = False
        else:
            self._opacity = max(0.3, self._opacity - step)
            if self._opacity <= 0.3:
                self._growing = True
        self.update()

    def paintEvent(self, event) -> None:
        from PySide6.QtGui import QPainter, QBrush
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        if self._active:
            color = QColor("#34d399")
            color.setAlphaF(self._opacity)
            painter.setBrush(QBrush(color))
            painter.setPen(Qt.PenStyle.NoPen)
            painter.drawEllipse(2, 2, 8, 8)
        else:
            painter.setBrush(QBrush(QColor("#334155")))
            painter.setPen(Qt.PenStyle.NoPen)
            painter.drawEllipse(2, 2, 8, 8)


def _section_header(icon: str, title: str) -> tuple[QWidget, QLabel]:
    """Return (header_widget, count_label)."""
    row = QWidget()
    layout = QHBoxLayout(row)
    layout.setContentsMargins(2, 0, 2, 4)
    layout.setSpacing(6)

    icon_lbl = QLabel(icon)
    icon_lbl.setStyleSheet("font-size:11px;background:transparent;")

    title_lbl = QLabel(title.upper())
    title_lbl.setObjectName("section_title")

    count_lbl = QLabel("")
    count_lbl.setObjectName("section_count")
    count_lbl.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)

    layout.addWidget(icon_lbl)
    layout.addWidget(title_lbl)
    layout.addStretch()
    layout.addWidget(count_lbl)
    return row, count_lbl


class MainWindow(QMainWindow):
    segment_received = Signal(object)
    result_received = Signal(object)
    asr_status_received = Signal(str)
    state_received = Signal(object)
    notes_requested = Signal()

    def __init__(self):
        super().__init__()
        self.setWindowTitle("TalkSage")
        self.setMinimumWidth(340)
        self.setMaximumWidth(420)
        self.setStyleSheet(STYLESHEET)
        self._term_count = 0
        self._listening = False
        self._asr_status = "待机"

        central = QWidget()
        central.setObjectName("central")
        self.setCentralWidget(central)
        root = QVBoxLayout(central)
        root.setContentsMargins(0, 0, 0, 0)
        root.setSpacing(0)

        # ── Title bar ────────────────────────────────────────
        titlebar = QWidget()
        titlebar.setObjectName("titlebar")
        titlebar.setFixedHeight(44)
        tb_layout = QHBoxLayout(titlebar)
        tb_layout.setContentsMargins(14, 0, 14, 0)

        self._dot = _PulseDot()
        app_name = QLabel("TalkSage")
        app_name.setObjectName("app_name")

        self._status_badge = QLabel("待机")
        self._status_badge.setObjectName("status_badge")
        self._status_badge.setProperty("active", "false")

        tb_layout.addWidget(self._dot)
        tb_layout.addSpacing(6)
        tb_layout.addWidget(app_name)
        tb_layout.addStretch()
        tb_layout.addWidget(self._status_badge)
        root.addWidget(titlebar)

        # ── Content area ──────────────────────────────────────
        content = QWidget()
        content_layout = QVBoxLayout(content)
        content_layout.setContentsMargins(12, 12, 12, 12)
        content_layout.setSpacing(10)

        # Record + notes buttons
        btn_row = QWidget()
        btn_layout = QHBoxLayout(btn_row)
        btn_layout.setContentsMargins(0, 0, 0, 0)
        btn_layout.setSpacing(8)
        self._record_btn = QPushButton("▶  开始监听")
        self._record_btn.setObjectName("record_btn")
        self._record_btn.setCheckable(True)
        self._record_btn.setFixedHeight(40)
        self._record_btn.toggled.connect(self._on_record_toggled)

        self._notes_btn = QPushButton("生成纪要")
        self._notes_btn.setObjectName("notes_btn")
        self._notes_btn.setFixedHeight(40)
        self._notes_btn.setEnabled(False)
        self._notes_btn.clicked.connect(self.notes_requested.emit)

        btn_layout.addWidget(self._record_btn, stretch=2)
        btn_layout.addWidget(self._notes_btn, stretch=1)
        content_layout.addWidget(btn_row)

        # Context
        c_header, _ = _section_header("🧭", "上下文")
        content_layout.addWidget(c_header)
        self.context_section = ContextSection()
        content_layout.addWidget(self.context_section)

        # Transcript
        t_header, _ = _section_header("🎙", "实时转写")
        content_layout.addWidget(t_header)
        self.transcript_section = TranscriptSection()
        content_layout.addWidget(self.transcript_section)

        # Terms
        tm_header, self._term_count_lbl = _section_header("📖", "术语")
        content_layout.addWidget(tm_header)
        self.terms_section = TermsSection()
        content_layout.addWidget(self.terms_section)

        # Suggestions / brief
        s_header, _ = _section_header("💡", "简报")
        content_layout.addWidget(s_header)
        self.suggestions_section = SuggestionsSection()
        content_layout.addWidget(self.suggestions_section)

        content_layout.addStretch()
        root.addWidget(content)

        # Signals
        self.segment_received.connect(self.transcript_section.add_segment)
        self.result_received.connect(self._route_result)
        self.asr_status_received.connect(self._on_asr_status)
        self.state_received.connect(self.context_section.set_state)

    def set_record_checked(self, checked: bool) -> None:
        """Update record button without emitting toggled (for consent cancel)."""
        self._record_btn.blockSignals(True)
        self._record_btn.setChecked(checked)
        self._record_btn.blockSignals(False)
        self._on_record_toggled(checked)

    def set_notes_enabled(self, enabled: bool) -> None:
        self._notes_btn.setEnabled(enabled)

    def set_notes_status(self, text: str) -> None:
        self._notes_btn.setText(text)

    @Slot(str)
    def _on_asr_status(self, message: str) -> None:
        self._asr_status = message
        if not self._listening:
            self._status_badge.setText(message)
            loading = "加载" in message or "失败" in message
            self._status_badge.setProperty("active", "false" if loading else "true")
            self._status_badge.style().unpolish(self._status_badge)
            self._status_badge.style().polish(self._status_badge)

    @Slot(bool)
    def _on_record_toggled(self, checked: bool) -> None:
        self._listening = checked
        self._record_btn.setText("⏹  停止监听" if checked else "▶  开始监听")
        self._dot.set_active(checked)
        if checked:
            self._status_badge.setText("● 监听中")
            self._status_badge.setProperty("active", "true")
            self._notes_btn.setEnabled(False)
            self._notes_btn.setText("生成纪要")
        else:
            self._status_badge.setText(self._asr_status if self._asr_status else "待机")
            ready = "就绪" in self._asr_status
            self._status_badge.setProperty("active", "true" if ready else "false")
            self._notes_btn.setEnabled(True)
        self._status_badge.style().unpolish(self._status_badge)
        self._status_badge.style().polish(self._status_badge)

    @Slot(object)
    def _route_result(self, result: PluginResult) -> None:
        if result.ui_section == "terms" and result.content:
            is_update = bool(result.result_id) and result.status == "final"
            self.terms_section.add_result(result)
            if not is_update:
                self._term_count += 1
                self._term_count_lbl.setText(str(self._term_count))
        elif result.ui_section == "suggestions" and result.content:
            self.suggestions_section.add_result(result)
