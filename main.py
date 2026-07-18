import sys
import asyncio
import threading
from PySide6.QtCore import QTimer
from PySide6.QtWidgets import QApplication, QMessageBox
from ui.main_window import MainWindow
from ui.consent_dialog import ensure_recording_consent
from ui.screen_share import set_exclude_from_capture
from ui.setup_wizard import maybe_run_setup_wizard
from core.audio_hub import AudioHub
from core.asr.factory import build_asr_engine
from core.plugin_bus import PluginBus
from core.pipeline import Pipeline
from core.session_store import SessionStore
from core.echo_filter import CrosstalkFilter
from core.knowledge_base import KnowledgeBase
from core.notes_generator import NotesGenerator
from plugins.term_explainer import TermExplainerPlugin
from plugins.brief_retriever import BriefRetrieverPlugin
from llm.openai_compat import OpenAICompatProvider
from config.manager import ConfigManager


def _make_llm(config: ConfigManager, provider_name: str | None = None) -> OpenAICompatProvider:
    name = provider_name or config.get("llm.default", "deepseek")
    provider_cfg = config.get_llm_provider(name)
    return OpenAICompatProvider(
        api_key=provider_cfg.get("api_key", ""),
        model=provider_cfg.get("model", "deepseek-chat"),
        base_url=provider_cfg.get("base_url"),
    )


def build_pipeline(config: ConfigManager) -> Pipeline:
    hub = AudioHub()
    engine = build_asr_engine(config.get("transcribe", {}))

    bus = PluginBus()
    if config.get("plugins.term_explainer.enabled"):
        provider_name = config.get("plugins.term_explainer.llm", "deepseek")
        llm = _make_llm(config, provider_name)
        cooldown = float(config.get("plugins.term_explainer.cooldown_seconds", 10) or 10)
        bus.register(TermExplainerPlugin(llm=llm, cooldown_seconds=cooldown))

    kb = KnowledgeBase()
    if config.get("knowledge_base.enabled"):
        folder = config.get("knowledge_base.folder") or ""
        if folder:
            kb.index_folder(folder)
        bus.context.knowledge_base = kb
        if kb.chunk_count > 0:
            bus.register(BriefRetrieverPlugin(kb=kb))

    sessions_dir = config.config_dir / "sessions"
    session_store = SessionStore(sessions_dir=sessions_dir) if config.get("session.auto_save", True) else None
    echo = CrosstalkFilter(
        similarity_threshold=float(config.get("audio.crosstalk.similarity_threshold", 0.6) or 0.6),
        window_seconds=float(config.get("audio.crosstalk.window_seconds", 8) or 8),
    )

    pipeline = Pipeline(
        hub=hub,
        engine=engine,
        bus=bus,
        echo_filter=echo,
        session_store=session_store,
    )
    pipeline._loopback_device = config.get("transcribe.loopback.device")
    return pipeline


def main():
    app = QApplication(sys.argv)
    config = ConfigManager()

    maybe_run_setup_wizard(config, parent=None)

    window = MainWindow()
    pipeline = build_pipeline(config)
    notes_llm = _make_llm(config, config.get("session.notes_llm") or config.get("llm.default"))
    notes_gen = NotesGenerator(llm=notes_llm)

    pipeline.on_segment = lambda seg: window.segment_received.emit(seg)
    pipeline.on_result = lambda res: window.result_received.emit(res)
    pipeline.on_asr_status = lambda msg: window.asr_status_received.emit(msg)
    pipeline.on_state = lambda state: window.state_received.emit(state)

    loopback_device = config.get("transcribe.loopback.device")

    def on_record_toggled(checked: bool) -> None:
        if checked:
            if not ensure_recording_consent(config, parent=window):
                window.set_record_checked(False)
                return
            pipeline.start(loopback_device=loopback_device)
        else:
            pipeline.stop()

    def on_notes_requested() -> None:
        window.set_notes_enabled(False)
        window.set_notes_status("生成中…")

        def worker() -> None:
            try:
                terms = pipeline.sessions.terms() if pipeline.sessions else []
                notes = asyncio.run(
                    notes_gen.generate(pipeline.bus.context, terms=terms)
                )
                path = None
                if pipeline.sessions is not None:
                    path = pipeline.sessions.append_notes(notes)

                def done() -> None:
                    window.set_notes_status("生成纪要")
                    window.set_notes_enabled(True)
                    msg = "纪要已生成"
                    if path:
                        msg += f"\n\n已写入:\n{path}"
                    QMessageBox.information(window, "会议纪要", msg)

                QTimer.singleShot(0, done)
            except Exception as exc:
                def fail() -> None:
                    window.set_notes_status("生成纪要")
                    window.set_notes_enabled(True)
                    QMessageBox.warning(window, "生成失败", str(exc))

                QTimer.singleShot(0, fail)

        threading.Thread(target=worker, daemon=True).start()

    window._record_btn.toggled.connect(on_record_toggled)
    window.notes_requested.connect(on_notes_requested)

    window.show()

    if config.get("privacy.hide_from_screen_share", True):
        QTimer.singleShot(0, lambda: set_exclude_from_capture(window, True))

    threading.Thread(target=pipeline.warmup, daemon=True).start()

    sys.exit(app.exec())


if __name__ == "__main__":
    main()
