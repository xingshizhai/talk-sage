from unittest.mock import patch, MagicMock
from core.device_probe import (
    detect_compute_device,
    list_input_devices,
    recommend_local_asr_settings,
    AudioDeviceInfo,
)


def test_detect_compute_device_cuda_when_available():
    with patch("core.device_probe._cuda_available", return_value=True):
        assert detect_compute_device() == "cuda"


def test_detect_compute_device_cpu_fallback():
    with patch("core.device_probe._cuda_available", return_value=False):
        assert detect_compute_device() == "cpu"


def test_recommend_local_asr_settings_for_cuda():
    settings = recommend_local_asr_settings("cuda")
    assert settings["device"] == "cuda"
    assert settings["compute_type"] == "float16"


def test_recommend_local_asr_settings_for_cpu():
    settings = recommend_local_asr_settings("cpu")
    assert settings["device"] == "cpu"
    assert settings["compute_type"] == "int8"


def test_list_input_devices_maps_sounddevice(monkeypatch):
    fake = [
        {"name": "Mic", "max_input_channels": 1, "max_output_channels": 0},
        {"name": "Speakers", "max_input_channels": 0, "max_output_channels": 2},
        {"name": "Stereo Mix", "max_input_channels": 2, "max_output_channels": 0},
    ]
    monkeypatch.setattr("core.device_probe._query_devices", lambda: fake)
    devices = list_input_devices()
    assert len(devices) == 2
    assert devices[0] == AudioDeviceInfo(index=0, name="Mic", is_loopback_candidate=False)
    assert devices[1].is_loopback_candidate is True
