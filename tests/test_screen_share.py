from ui.screen_share import set_exclude_from_capture, is_exclude_supported


def test_is_exclude_supported_returns_bool():
    assert isinstance(is_exclude_supported(), bool)


def test_set_exclude_from_capture_accepts_none_widget():
    # Should not raise when widget has no native handle yet
    assert set_exclude_from_capture(None, True) is False
