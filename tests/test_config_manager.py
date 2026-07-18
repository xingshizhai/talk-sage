import pytest
import tempfile
from pathlib import Path
from config.manager import ConfigManager


@pytest.fixture
def tmp_config_dir(tmp_path):
    return tmp_path / ".talksage"


def test_loads_defaults_when_no_file(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("transcribe.client.model") == "small"
    assert mgr.get("transcribe.user.model") == "paraformer-zh"
    assert mgr.get("transcribe.client.device") == "auto"
    assert mgr.get("audio.ducking.enabled") is True


def test_creates_config_dir_on_save(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    mgr.save()
    assert (tmp_config_dir / "config.yaml").exists()


def test_get_nested_key(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("plugins.term_explainer.enabled") is True


def test_set_and_get(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    mgr.set("transcribe.client.model", "medium")
    assert mgr.get("transcribe.client.model") == "medium"


def test_get_missing_key_returns_default(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("nonexistent.key", default="fallback") == "fallback"


def test_get_llm_provider_config(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    provider = mgr.get_llm_provider("deepseek")
    assert provider["model"] == "deepseek-chat"
    assert "base_url" in provider


def test_privacy_defaults(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("privacy.recording_consent_accepted") is False
    assert mgr.get("privacy.hide_from_screen_share") is True


def test_term_explainer_cooldown_default(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("plugins.term_explainer.cooldown_seconds") == 10


def test_transcribe_mode_default_local(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("transcribe.mode") == "local"
    assert mgr.get("session.auto_save") is True
