from __future__ import annotations

from PySide6.QtWidgets import (
    QVBoxLayout,
    QLabel,
    QWizard,
    QWizardPage,
    QComboBox,
    QLineEdit,
    QFormLayout,
    QWidget,
    QFileDialog,
    QPushButton,
    QHBoxLayout,
)
from config.manager import ConfigManager

_ASR_MODES = [("local", "本地双引擎（faster-whisper + FunASR）"), ("cloud", "云端 Whisper API")]
_LLM_PROVIDERS = ["deepseek", "groq", "kimi", "minimax", "ollama", "claude"]


class _WelcomePage(QWizardPage):
    def __init__(self):
        super().__init__()
        self.setTitle("欢迎使用 TalkSage")
        layout = QVBoxLayout(self)
        layout.addWidget(QLabel(
            "几步完成初始设置：选择语音识别方式、配置 LLM，以及可选的客户简报知识库。\n"
            "之后可随时编辑 ~/.talksage/config.yaml。"
        ))
        layout.addStretch()


class _AsrPage(QWizardPage):
    def __init__(self):
        super().__init__()
        self.setTitle("语音识别")
        layout = QFormLayout(self)
        self.mode = QComboBox()
        for value, label in _ASR_MODES:
            self.mode.addItem(label, value)
        layout.addRow("模式", self.mode)
        hint = QLabel("本地模式首次会下载模型；云端模式需填写 OpenAI 兼容 API Key（可稍后在配置文件中设置）。")
        hint.setWordWrap(True)
        layout.addRow(hint)


class _LlmPage(QWizardPage):
    def __init__(self):
        super().__init__()
        self.setTitle("大模型")
        layout = QFormLayout(self)
        self.provider = QComboBox()
        for name in _LLM_PROVIDERS:
            self.provider.addItem(name)
        self.api_key = QLineEdit()
        self.api_key.setEchoMode(QLineEdit.EchoMode.Password)
        self.api_key.setPlaceholderText("API Key（Ollama 可留空）")
        layout.addRow("提供商", self.provider)
        layout.addRow("API Key", self.api_key)


class _KbPage(QWizardPage):
    def __init__(self):
        super().__init__()
        self.setTitle("客户简报（可选）")
        layout = QVBoxLayout(self)
        layout.addWidget(QLabel(
            "选择包含 .md / .txt 的文件夹（客户档案、报价、历史纪要等）。\n"
            "会议中会按关键词检索并显示相关片段。可跳过。"
        ))
        row = QHBoxLayout()
        self.folder = QLineEdit()
        self.folder.setPlaceholderText("知识库文件夹路径")
        browse = QPushButton("浏览…")
        browse.clicked.connect(self._browse)
        row.addWidget(self.folder)
        row.addWidget(browse)
        layout.addLayout(row)
        layout.addStretch()

    def _browse(self) -> None:
        path = QFileDialog.getExistingDirectory(self, "选择知识库文件夹")
        if path:
            self.folder.setText(path)


class SetupWizard(QWizard):
    def __init__(self, config: ConfigManager, parent: QWidget | None = None):
        super().__init__(parent)
        self._config = config
        self.setWindowTitle("TalkSage 初始设置")
        self.setMinimumWidth(480)
        self._welcome = _WelcomePage()
        self._asr = _AsrPage()
        self._llm = _LlmPage()
        self._kb = _KbPage()
        self.addPage(self._welcome)
        self.addPage(self._asr)
        self.addPage(self._llm)
        self.addPage(self._kb)
        self.button(QWizard.WizardButton.FinishButton).clicked.connect(self.apply)

        # Prefill
        mode = config.get("transcribe.mode", "local")
        idx = self._asr.mode.findData(mode)
        if idx >= 0:
            self._asr.mode.setCurrentIndex(idx)
        provider = config.get("llm.default", "deepseek")
        pidx = self._llm.provider.findText(provider)
        if pidx >= 0:
            self._llm.provider.setCurrentIndex(pidx)
        folder = config.get("knowledge_base.folder") or ""
        self._kb.folder.setText(str(folder))

    def page_count(self) -> int:
        return len(self.pageIds())

    def set_asr_mode(self, mode: str) -> None:
        idx = self._asr.mode.findData(mode)
        if idx >= 0:
            self._asr.mode.setCurrentIndex(idx)

    def set_llm_provider(self, name: str) -> None:
        idx = self._llm.provider.findText(name)
        if idx >= 0:
            self._llm.provider.setCurrentIndex(idx)

    def set_llm_api_key(self, key: str) -> None:
        self._llm.api_key.setText(key)

    def set_kb_folder(self, path: str) -> None:
        self._kb.folder.setText(path)

    def apply(self) -> None:
        mode = self._asr.mode.currentData()
        provider = self._llm.provider.currentText()
        api_key = self._llm.api_key.text().strip()
        folder = self._kb.folder.text().strip()

        self._config.set("transcribe.mode", mode)
        self._config.set("llm.default", provider)
        self._config.set("plugins.term_explainer.llm", provider)
        if api_key:
            self._config.set(f"llm.providers.{provider}.api_key", api_key)
        if folder:
            self._config.set("knowledge_base.enabled", True)
            self._config.set("knowledge_base.folder", folder)
        else:
            self._config.set("knowledge_base.enabled", False)
        self._config.set("setup.completed", True)
        self._config.save()


def maybe_run_setup_wizard(config: ConfigManager, parent: QWidget | None = None) -> None:
    if config.get("setup.completed"):
        return
    wizard = SetupWizard(config=config, parent=parent)
    wizard.exec()
