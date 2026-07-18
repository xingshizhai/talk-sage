import pytest
from PySide6.QtWidgets import QApplication, QDialog
from ui.consent_dialog import RecordingConsentDialog, ensure_recording_consent


@pytest.fixture(scope="session")
def qapp():
    return QApplication.instance() or QApplication([])


def test_consent_dialog_has_accept_and_reject(qapp, qtbot):
    dlg = RecordingConsentDialog()
    qtbot.addWidget(dlg)
    assert dlg.accept_button is not None
    assert dlg.reject_button is not None


def test_ensure_consent_returns_true_when_already_accepted(qapp, tmp_path):
    from config.manager import ConfigManager

    mgr = ConfigManager(config_dir=tmp_path / ".talksage")
    mgr.set("privacy.recording_consent_accepted", True)
    assert ensure_recording_consent(mgr, parent=None) is True


def test_ensure_consent_persists_on_accept(qapp, qtbot, tmp_path, monkeypatch):
    from config.manager import ConfigManager

    mgr = ConfigManager(config_dir=tmp_path / ".talksage")
    assert mgr.get("privacy.recording_consent_accepted") is False

    def fake_exec(self):
        return QDialog.DialogCode.Accepted

    monkeypatch.setattr(RecordingConsentDialog, "exec", fake_exec)
    assert ensure_recording_consent(mgr, parent=None) is True
    assert mgr.get("privacy.recording_consent_accepted") is True
    assert (tmp_path / ".talksage" / "config.yaml").exists()
