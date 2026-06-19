from PySide6.QtWidgets import QListWidget, QListWidgetItem
from PySide6.QtCore import Slot
from core.models import PluginResult


class TermsSection(QListWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMaximumHeight(200)

    @Slot(object)
    def add_result(self, result: PluginResult) -> None:
        item = QListWidgetItem(result.content)
        self.insertItem(0, item)  # newest at top
        if self.count() > 20:
            self.takeItem(self.count() - 1)
