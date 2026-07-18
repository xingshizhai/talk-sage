from PySide6.QtWidgets import (
    QScrollArea, QWidget, QVBoxLayout, QFrame, QLabel
)
from PySide6.QtCore import Slot, Qt
from core.models import PluginResult


def _make_suggestion_card(content: str) -> QFrame:
    card = QFrame()
    card.setObjectName("suggestion_card")
    layout = QVBoxLayout(card)
    layout.setContentsMargins(10, 8, 10, 8)
    layout.setSpacing(4)

    label = QLabel("简报")
    label.setObjectName("suggestion_label")

    body = QLabel(content)
    body.setObjectName("suggestion_text")
    body.setWordWrap(True)

    layout.addWidget(label)
    layout.addWidget(body)
    return card


class SuggestionsSection(QScrollArea):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("suggestions_scroll")
        self.setWidgetResizable(True)
        self.setMinimumHeight(60)
        self.setMaximumHeight(160)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)

        self._container = QWidget()
        self._layout = QVBoxLayout(self._container)
        self._layout.setContentsMargins(0, 0, 0, 0)
        self._layout.setSpacing(6)
        self._layout.addStretch()
        self.setWidget(self._container)
        self._count = 0

    def count(self) -> int:
        return self._count

    @Slot(object)
    def add_result(self, result: PluginResult) -> None:
        if not result.content:
            return
        card = _make_suggestion_card(result.content)
        self._layout.insertWidget(0, card)
        self._count += 1
        if self._count > 10:
            last = self._layout.itemAt(self._count - 1)
            if last and last.widget():
                last.widget().deleteLater()
                self._count -= 1
