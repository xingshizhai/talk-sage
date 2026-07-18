from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QDialog,
    QVBoxLayout,
    QHBoxLayout,
    QLineEdit,
    QPushButton,
    QListWidget,
    QListWidgetItem,
    QLabel,
    QTextBrowser,
    QWidget,
)
from core.session_db import SessionDatabase

_USER_ROLE = Qt.ItemDataRole.UserRole


class HistoryDialog(QDialog):
    """Browse / search past sessions stored in SQLite."""

    def __init__(self, db: SessionDatabase, parent: QWidget | None = None):
        super().__init__(parent)
        self._db = db
        self.setWindowTitle("会话历史")
        self.setMinimumSize(480, 360)

        layout = QVBoxLayout(self)
        row = QHBoxLayout()
        self.query = QLineEdit()
        self.query.setPlaceholderText("搜索转写关键词，如 BOQ / NPI")
        search_btn = QPushButton("搜索")
        search_btn.clicked.connect(self._search)
        refresh_btn = QPushButton("最近会话")
        refresh_btn.clicked.connect(self._load_recent)
        row.addWidget(self.query)
        row.addWidget(search_btn)
        row.addWidget(refresh_btn)
        layout.addLayout(row)

        self.list = QListWidget()
        self.list.currentItemChanged.connect(self._show_detail)
        layout.addWidget(self.list, stretch=1)

        self.detail = QTextBrowser()
        self.detail.setMinimumHeight(140)
        layout.addWidget(self.detail)

        layout.addWidget(QLabel("数据来自 ~/.talksage/sessions.db；Markdown 仍在 sessions/ 目录。"))
        self._load_recent()

    def _load_recent(self) -> None:
        self.list.clear()
        for s in self._db.list_sessions(limit=40):
            item = QListWidgetItem(
                f"{s['stamp']}  ·  {s['segment_count']} 段"
            )
            item.setData(_USER_ROLE, s["id"])
            self.list.addItem(item)

    def _search(self) -> None:
        q = self.query.text().strip()
        self.list.clear()
        if not q:
            self._load_recent()
            return
        for hit in self._db.search(q, limit=40):
            item = QListWidgetItem(
                f"[{hit['stamp']}] {hit['speaker']}: {hit['text'][:80]}"
            )
            item.setData(_USER_ROLE, hit["session_id"])
            self.list.addItem(item)

    def _show_detail(self, current: QListWidgetItem | None, _prev) -> None:
        if current is None:
            self.detail.clear()
            return
        sid = current.data(_USER_ROLE)
        try:
            detail = self._db.get_session(int(sid))
        except KeyError:
            self.detail.setPlainText("会话不存在")
            return
        lines = [
            f"# {detail['stamp']}",
            f"Markdown: {detail.get('markdown_path') or '—'}",
            "",
            "## 转写",
        ]
        for seg in detail["segments"][:30]:
            lines.append(f"- [{seg['speaker']}] {seg['text']}")
        if detail.get("terms"):
            lines.extend(["", "## 术语"])
            lines.extend(f"- {t}" for t in detail["terms"])
        if detail.get("notes"):
            lines.extend(["", "## 纪要", detail["notes"]])
        self.detail.setPlainText("\n".join(lines))
