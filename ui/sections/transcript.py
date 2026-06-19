from PySide6.QtWidgets import QTextEdit
from PySide6.QtCore import Slot
from core.models import TranscriptSegment


class TranscriptSection(QTextEdit):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setReadOnly(True)
        self.setMaximumHeight(150)

    @Slot(object)
    def add_segment(self, segment: TranscriptSegment) -> None:
        label = f"[{segment.speaker}]"
        self.append(f"{label} {segment.text}")
        self.ensureCursorVisible()
