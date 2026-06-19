import sys
from PySide6.QtWidgets import QApplication
from ui.main_window import MainWindow
from core.audio_hub import AudioHub
from core.transcribe_engine import TranscribeEngine
from core.plugin_bus import PluginBus
from core.pipeline import Pipeline
from plugins.term_explainer import TermExplainerPlugin
from llm.openai_compat import OpenAICompatProvider
from config.manager import ConfigManager


def build_pipeline(config: ConfigManager) -> Pipeline:
    hub = AudioHub()
    engine = TranscribeEngine(model_size=config.get("transcribe.model", "base"))
    bus = PluginBus()

    if config.get("plugins.term_explainer.enabled"):
        provider_name = config.get("plugins.term_explainer.llm", "deepseek")
        provider_cfg = config.get_llm_provider(provider_name)
        llm = OpenAICompatProvider(
            api_key=provider_cfg.get("api_key", ""),
            model=provider_cfg.get("model", "deepseek-chat"),
            base_url=provider_cfg.get("base_url"),
        )
        bus.register(TermExplainerPlugin(llm=llm))

    return Pipeline(hub=hub, engine=engine, bus=bus)


def main():
    app = QApplication(sys.argv)
    config = ConfigManager()
    window = MainWindow()

    pipeline = build_pipeline(config)
    pipeline.on_segment = lambda seg: window.segment_received.emit(seg)
    pipeline.on_result = lambda res: window.result_received.emit(res)

    window._record_btn.toggled.connect(
        lambda checked: pipeline.start() if checked else pipeline.stop()
    )

    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
