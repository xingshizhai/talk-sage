import pytest
from PySide6.QtWidgets import QApplication, QDialog
from ui.setup_wizard import SetupWizard, maybe_run_setup_wizard
from config.manager import ConfigManager


@pytest.fixture(scope="session")
def qapp():
    return QApplication.instance() or QApplication([])


def test_wizard_pages_and_apply(qapp, qtbot, tmp_path):
    mgr = ConfigManager(config_dir=tmp_path / ".talksage")
    wizard = SetupWizard(config=mgr)
    qtbot.addWidget(wizard)
    assert wizard.page_count() >= 3

    wizard.set_asr_mode("cloud")
    wizard.set_llm_provider("deepseek")
    wizard.set_llm_api_key("sk-test-key")
    wizard.set_kb_folder(str(tmp_path / "briefs"))
    wizard.apply()

    assert mgr.get("transcribe.mode") == "cloud"
    assert mgr.get("llm.providers.deepseek.api_key") == "sk-test-key"
    assert mgr.get("knowledge_base.enabled") is True
    assert mgr.get("setup.completed") is True


def test_maybe_run_skips_when_completed(qapp, tmp_path, monkeypatch):
    mgr = ConfigManager(config_dir=tmp_path / ".talksage")
    mgr.set("setup.completed", True)

    called = []

    class FakeWizard:
        def __init__(self, *a, **k):
            called.append(True)

        def exec(self):
            return QDialog.DialogCode.Accepted

    monkeypatch.setattr("ui.setup_wizard.SetupWizard", FakeWizard)
    maybe_run_setup_wizard(mgr, parent=None)
    assert called == []


def test_maybe_run_shows_when_not_completed(qapp, tmp_path, monkeypatch):
    mgr = ConfigManager(config_dir=tmp_path / ".talksage")
    assert mgr.get("setup.completed") is False

    class FakeWizard:
        def __init__(self, config, parent=None):
            self.config = config

        def exec(self):
            self.config.set("setup.completed", True)
            self.config.save()
            return QDialog.DialogCode.Accepted

    monkeypatch.setattr("ui.setup_wizard.SetupWizard", FakeWizard)
    maybe_run_setup_wizard(mgr, parent=None)
    assert mgr.get("setup.completed") is True
