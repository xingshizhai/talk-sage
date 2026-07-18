from PySide6.QtWidgets import QTextEdit
from PySide6.QtCore import Slot
from core.models import ConversationState


class ContextSection(QTextEdit):
    """Shows live conversation state (topic / open questions / decisions)."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("context")
        self.setReadOnly(True)
        self.setMinimumHeight(60)
        self.setMaximumHeight(110)
        self.setPlaceholderText("对话上下文将在客户发言后更新…")

    @Slot(object)
    def set_state(self, state: ConversationState) -> None:
        self.setPlainText(state.as_brief())
