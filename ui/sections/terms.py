from PySide6.QtWidgets import (
    QScrollArea, QWidget, QVBoxLayout, QFrame, QLabel, QSizePolicy
)
from PySide6.QtCore import Slot, Qt
from core.models import PluginResult


def _make_term_card(content: str) -> QFrame:
    """Build a styled card for a single term result."""
    # Try to split "TERM = definition" for visual emphasis
    if " = " in content:
        term, _, definition = content.partition(" = ")
    else:
        term, definition = content, ""

    card = QFrame()
    card.setStyleSheet(
        "QFrame {"
        "  background:#080c14;"
        "  border:1px solid rgba(255,255,255,0.06);"
        "  border-left:3px solid #2dd4bf;"
        "  border-radius:10px;"
        "  padding:0;"
        "}"
    )
    layout = QVBoxLayout(card)
    layout.setContentsMargins(10, 8, 10, 8)
    layout.setSpacing(3)

    term_lbl = QLabel(term.strip())
    term_lbl.setStyleSheet(
        "color:#2dd4bf;"
        "font-size:12px;"
        "font-weight:600;"
        "font-family:'Courier New',Courier,monospace;"
        "background:transparent;"
        "border:none;"
    )

    def_lbl = QLabel(definition.strip() if definition else content)
    def_lbl.setWordWrap(True)
    def_lbl.setStyleSheet(
        "color:#94a3b8;"
        "font-size:11px;"
        "line-height:1.5;"
        "background:transparent;"
        "border:none;"
    )

    layout.addWidget(term_lbl)
    if definition:
        layout.addWidget(def_lbl)

    return card


class TermsSection(QScrollArea):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("terms_scroll")
        self.setWidgetResizable(True)
        self.setMinimumHeight(80)
        self.setMaximumHeight(240)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)

        self._container = QWidget()
        self._container.setObjectName("terms_container")
        self._layout = QVBoxLayout(self._container)
        self._layout.setContentsMargins(0, 0, 0, 0)
        self._layout.setSpacing(6)
        self._layout.addStretch()

        self.setWidget(self._container)
        self._count = 0

    def count(self) -> int:
        return self._count

    def item(self, index: int):
        """Return item at index (for test compatibility)."""
        # Cards are inserted before the stretch; index 0 = newest
        item_widget = self._layout.itemAt(index)
        if item_widget and item_widget.widget():
            return _CardProxy(item_widget.widget())
        return None

    @Slot(object)
    def add_result(self, result: PluginResult) -> None:
        card = _make_term_card(result.content)
        self._layout.insertWidget(0, card)
        self._count += 1
        if self._count > 20:
            # Remove oldest (last item before stretch)
            last = self._layout.itemAt(self._count - 1)
            if last and last.widget():
                last.widget().deleteLater()
                self._count -= 1


class _CardProxy:
    """Minimal proxy so tests can call .text() on a card."""
    def __init__(self, widget: QFrame):
        self._widget = widget

    def text(self) -> str:
        labels = self._widget.findChildren(QLabel)
        return " ".join(lbl.text() for lbl in labels)
