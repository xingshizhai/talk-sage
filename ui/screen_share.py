"""Best-effort exclusion of the app window from screen capture / screen share.

Windows 10 2004+ supports SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE).
macOS / Linux: not available via this helper; see README for user guidance.
"""

from __future__ import annotations

import sys
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from PySide6.QtWidgets import QWidget

_WDA_NONE = 0x00000000
_WDA_EXCLUDEFROMCAPTURE = 0x00000011


def is_exclude_supported() -> bool:
    return sys.platform == "win32"


def set_exclude_from_capture(widget: QWidget | None, enabled: bool) -> bool:
    """Exclude *widget* from screen capture when *enabled*.

    Returns True if the OS call succeeded, False if unsupported or failed.
    """
    if widget is None or not is_exclude_supported():
        return False
    try:
        import ctypes
        from ctypes import wintypes
    except Exception:
        return False

    hwnd = int(widget.winId())
    if hwnd == 0:
        return False

    user32 = ctypes.windll.user32
    SetWindowDisplayAffinity = user32.SetWindowDisplayAffinity
    SetWindowDisplayAffinity.argtypes = [wintypes.HWND, wintypes.DWORD]
    SetWindowDisplayAffinity.restype = wintypes.BOOL

    affinity = _WDA_EXCLUDEFROMCAPTURE if enabled else _WDA_NONE
    ok = bool(SetWindowDisplayAffinity(hwnd, affinity))
    return ok
