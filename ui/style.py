STYLESHEET = """
/* ── Global ─────────────────────────────────────────────── */
QMainWindow, QWidget#central {
    background-color: #0d1420;
    color: #e2e8f0;
}

QScrollBar:vertical {
    background: #080c14;
    width: 4px;
    border-radius: 2px;
}
QScrollBar::handle:vertical {
    background: #1e293b;
    border-radius: 2px;
    min-height: 20px;
}
QScrollBar::handle:vertical:hover { background: #2dd4bf; }
QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }

/* ── Title bar ───────────────────────────────────────────── */
QWidget#titlebar {
    background-color: #080c14;
    border-bottom: 1px solid rgba(255,255,255,0.06);
}
QLabel#app_name {
    color: #e2e8f0;
    font-size: 13px;
    font-weight: 600;
}
QLabel#status_badge {
    color: #64748b;
    font-size: 10px;
    font-weight: 500;
    padding: 2px 10px;
    border-radius: 10px;
    background: rgba(100,116,139,0.1);
    border: 1px solid rgba(100,116,139,0.2);
}
QLabel#status_badge[active="true"] {
    color: #2dd4bf;
    background: rgba(45,212,191,0.12);
    border: 1px solid rgba(45,212,191,0.25);
}

/* ── Record button ───────────────────────────────────────── */
QPushButton#record_btn {
    background: qlineargradient(x1:0,y1:0,x2:1,y2:1,
        stop:0 #1e3a5f, stop:1 #0f2340);
    color: #2dd4bf;
    border: 1px solid rgba(45,212,191,0.3);
    border-radius: 20px;
    padding: 8px 28px;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 0.3px;
}
QPushButton#record_btn:hover {
    background: qlineargradient(x1:0,y1:0,x2:1,y2:1,
        stop:0 #234568, stop:1 #112849);
    border-color: rgba(45,212,191,0.5);
}
QPushButton#record_btn:checked {
    background: qlineargradient(x1:0,y1:0,x2:1,y2:1,
        stop:0 #1a3a2e, stop:1 #0d2018);
    color: #34d399;
    border-color: rgba(52,211,153,0.4);
}
QPushButton#record_btn:checked:hover {
    background: qlineargradient(x1:0,y1:0,x2:1,y2:1,
        stop:0 #1f4535, stop:1 #10261e);
}

/* ── Section headers ─────────────────────────────────────── */
QLabel#section_title {
    color: #64748b;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 1.2px;
}
QLabel#section_count {
    color: #334155;
    font-size: 9px;
    font-family: 'Courier New', monospace;
}

/* ── Transcript / context boxes ──────────────────────────── */
QTextEdit#transcript, QTextEdit#context {
    background-color: #080c14;
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 10px;
    color: #94a3b8;
    font-family: 'Courier New', Courier, monospace;
    font-size: 11px;
    padding: 8px;
    selection-background-color: #1e3a5f;
}

QPushButton#notes_btn {
    background: #121826;
    color: #94a3b8;
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 20px;
    padding: 8px 12px;
    font-size: 12px;
    font-weight: 500;
}
QPushButton#notes_btn:hover { border-color: rgba(245,158,11,0.4); color: #f59e0b; }
QPushButton#notes_btn:disabled { color: #334155; border-color: rgba(255,255,255,0.04); }

/* ── Terms list ──────────────────────────────────────────── */
QScrollArea#terms_scroll {
    background: transparent;
    border: none;
}
QWidget#terms_container { background: transparent; }

/* ── Suggestion card ─────────────────────────────────────── */
QFrame#suggestion_card {
    background: #080c14;
    border: 1px solid rgba(255,255,255,0.06);
    border-left: 3px solid #f59e0b;
    border-radius: 10px;
}
QLabel#suggestion_label {
    color: #f59e0b;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.8px;
}
QLabel#suggestion_text {
    color: #94a3b8;
    font-size: 11px;
}
"""
