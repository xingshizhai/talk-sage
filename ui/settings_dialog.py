from __future__ import annotations

from PySide6.QtWidgets import (
    QDialog,
    QVBoxLayout,
    QFormLayout,
    QComboBox,
    QDialogButtonBox,
    QLabel,
    QCheckBox,
    QWidget,
)
from config.manager import ConfigManager
from core.device_probe import list_input_devices, detect_compute_device, recommend_local_asr_settings


class SettingsDialog(QDialog):
    """Audio device + local ASR device settings."""

    def __init__(self, config: ConfigManager, parent: QWidget | None = None):
        super().__init__(parent)
        self._config = config
        self.setWindowTitle("设置")
        self.setMinimumWidth(420)

        layout = QVBoxLayout(self)
        form = QFormLayout()

        gpu = detect_compute_device()
        rec = recommend_local_asr_settings(gpu)
        hint = QLabel(f"计算设备探测：{gpu.upper()}（推荐 compute_type={rec['compute_type']}）")
        hint.setWordWrap(True)
        layout.addWidget(hint)

        self.mic = QComboBox()
        self.loopback = QComboBox()
        self.mic.addItem("系统默认", None)
        self.loopback.addItem("自动检测", None)
        for d in list_input_devices():
            label = d.name
            if d.is_loopback_candidate:
                label += "  [回环]"
            self.mic.addItem(label, d.index)
            self.loopback.addItem(label, d.index)

        self.client_engine = QComboBox()
        self.client_engine.addItem("faster-whisper（默认）", "faster-whisper")
        self.client_engine.addItem("Parakeet（需 onnx-asr）", "parakeet")

        self.asr_device = QComboBox()
        self.asr_device.addItem("自动检测", "auto")
        self.asr_device.addItem("CPU", "cpu")
        self.asr_device.addItem("CUDA (GPU)", "cuda")

        self.ducking = QCheckBox("启用麦克风闪避（系统音响时压低麦）")
        self.soft_limit = QCheckBox("启用防削波")

        form.addRow("麦克风", self.mic)
        form.addRow("系统回环（客户）", self.loopback)
        form.addRow("英文 ASR 引擎", self.client_engine)
        form.addRow("ASR 计算设备", self.asr_device)
        layout.addLayout(form)
        layout.addWidget(self.ducking)
        layout.addWidget(self.soft_limit)

        buttons = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Save | QDialogButtonBox.StandardButton.Cancel
        )
        buttons.accepted.connect(self._save)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

        self._prefill()

    def _prefill(self) -> None:
        mic = self._config.get("audio.mic_device")
        lb = self._config.get("transcribe.loopback.device")
        self._select_by_data(self.mic, mic)
        self._select_by_data(self.loopback, lb)
        eng = self._config.get("transcribe.client.engine", "faster-whisper")
        eidx = self.client_engine.findData(eng)
        if eidx >= 0:
            self.client_engine.setCurrentIndex(eidx)
        device = self._config.get("transcribe.client.device", "auto")
        idx = self.asr_device.findData(device if device in ("auto", "cpu", "cuda") else "auto")
        if idx >= 0:
            self.asr_device.setCurrentIndex(idx)
        self.ducking.setChecked(bool(self._config.get("audio.ducking.enabled", True)))
        self.soft_limit.setChecked(bool(self._config.get("audio.soft_limit.enabled", True)))

    def _select_by_data(self, combo: QComboBox, value) -> None:
        for i in range(combo.count()):
            if combo.itemData(i) == value:
                combo.setCurrentIndex(i)
                return

    def _save(self) -> None:
        self._config.set("audio.mic_device", self.mic.currentData())
        self._config.set("transcribe.loopback.device", self.loopback.currentData())
        engine = self.client_engine.currentData()
        self._config.set("transcribe.client.engine", engine)
        if engine == "parakeet":
            self._config.set("transcribe.client.model", "nemo-parakeet-tdt-0.6b-v3")
        asr_dev = self.asr_device.currentData()
        self._config.set("transcribe.client.device", asr_dev)
        self._config.set("transcribe.user.device", asr_dev)
        if asr_dev == "auto":
            self._config.set("transcribe.client.compute_type", "auto")
        elif asr_dev == "cuda":
            self._config.set("transcribe.client.compute_type", "float16")
        else:
            self._config.set("transcribe.client.compute_type", "int8")
        self._config.set("audio.ducking.enabled", self.ducking.isChecked())
        self._config.set("audio.soft_limit.enabled", self.soft_limit.isChecked())
        self._config.save()
        self.accept()
