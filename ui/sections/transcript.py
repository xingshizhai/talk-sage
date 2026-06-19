from PySide6.QtWidgets import QTextEdit
from PySide6.QtCore import Slot
from PySide6.QtGui import QTextCursor
from core.models import TranscriptSegment

_SPEAKER_STYLE = {
    "client": ("客户", "#2dd4bf", "rgba(45,212,191,0.12)"),
    "user":   ("我",   "#818cf8", "rgba(99,102,241,0.12)"),
}


class TranscriptSection(QTextEdit):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("transcript")
        self.setReadOnly(True)
        self.setMinimumHeight(120)
        self.setMaximumHeight(180)

    @Slot(object)
    def add_segment(self, segment: TranscriptSegment) -> None:
        label, color, bg = _SPEAKER_STYLE.get(
            segment.speaker, ("?", "#64748b", "rgba(100,116,139,0.1)")
        )
        badge = (
            f'<span style="'
            f'font-size:9px;font-weight:600;'
            f'color:{color};background:{bg};'
            f'padding:1px 5px;border-radius:3px;'
            f'font-family:\'Courier New\',monospace;'
            f'">{label}</span>'
        )
        text_html = (
            f'<span style="color:#94a3b8;font-size:11px;'
            f'font-family:\'Courier New\',monospace;">'
            f'{segment.text}</span>'
        )
        self.append(f'{badge}&nbsp;&nbsp;{text_html}')
        self.moveCursor(QTextCursor.MoveOperation.End)
