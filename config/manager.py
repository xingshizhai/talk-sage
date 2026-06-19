from pathlib import Path
from typing import Any
import yaml


_DEFAULTS_PATH = Path(__file__).parent / "defaults.yaml"


def _deep_merge(base: dict, override: dict) -> dict:
    result = base.copy()
    for key, value in override.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = _deep_merge(result[key], value)
        else:
            result[key] = value
    return result


class ConfigManager:
    def __init__(self, config_dir: Path | None = None):
        self._dir = config_dir or Path.home() / ".talksage"
        self._path = self._dir / "config.yaml"
        with open(_DEFAULTS_PATH) as f:
            self._data: dict = yaml.safe_load(f)
        if self._path.exists():
            with open(self._path) as f:
                user_data = yaml.safe_load(f) or {}
            self._data = _deep_merge(self._data, user_data)

    def get(self, key: str, default: Any = None) -> Any:
        parts = key.split(".")
        node = self._data
        for part in parts:
            if not isinstance(node, dict) or part not in node:
                return default
            node = node[part]
        return node

    def set(self, key: str, value: Any) -> None:
        parts = key.split(".")
        node = self._data
        for part in parts[:-1]:
            node = node.setdefault(part, {})
        node[parts[-1]] = value

    def save(self) -> None:
        self._dir.mkdir(parents=True, exist_ok=True)
        with open(self._path, "w") as f:
            yaml.dump(self._data, f, allow_unicode=True, default_flow_style=False)

    def get_llm_provider(self, name: str) -> dict:
        return self.get(f"llm.providers.{name}", default={})
