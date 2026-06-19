import pytest
import tempfile
from pathlib import Path
from config.manager import ConfigManager


@pytest.fixture
def tmp_config_dir(tmp_path):
    return tmp_path / ".talksage"


def test_loads_defaults_when_no_file(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("transcribe.mode") == "local"
    assert mgr.get("transcribe.model") == "base"


def test_creates_config_dir_on_save(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    mgr.save()
    assert (tmp_config_dir / "config.yaml").exists()


def test_get_nested_key(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("plugins.term_explainer.enabled") is True


def test_set_and_get(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    mgr.set("transcribe.mode", "api")
    assert mgr.get("transcribe.mode") == "api"


def test_get_missing_key_returns_default(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    assert mgr.get("nonexistent.key", default="fallback") == "fallback"


def test_get_llm_provider_config(tmp_config_dir):
    mgr = ConfigManager(config_dir=tmp_config_dir)
    provider = mgr.get_llm_provider("deepseek")
    assert provider["model"] == "deepseek-chat"
    assert "base_url" in provider
