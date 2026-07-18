import sys
import asyncio
import threading
from pathlib import Path
from PySide6.QtCore import QTimer
from PySide6.QtWidgets import QApplication, QMessageBox, QFileDialog
from ui.main_window import MainWindow
from ui.consent_dialog import ensure_recording_consent
from ui.screen_share import set_exclude_from_capture
from ui.setup_wizard import maybe_run_setup_wizard
from ui.settings_dialog import SettingsDialog
from ui.history_dialog import HistoryDialog
from core.audio_hub import AudioHub
from core.asr.factory import build_asr_engine
from core.plugin_bus import PluginBus
from core.pipeline import Pipeline
from core.session_store import SessionStore
from core.session_db import SessionDatabase
from core.echo_filter import CrosstalkFilter
from core.knowledge_base import KnowledgeBase
from core.notes_generator import NotesGenerator
from core.import_audio import load_audio_file, OfflineTranscriber
from core.device_probe import detect_compute_device
from core.models import TranscriptSegment
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
    hub = AudioHub(
        mic_device=config.get("audio.mic_device"),
        soft_limit_enabled=bool(config.get("audio.soft_limit.enabled", True)),
        ducking_enabled=bool(config.get("audio.ducking.enabled", True)),
        ducking_threshold=float(config.get("audio.ducking.threshold", 0.04) or 0.04),
        ducking_factor=float(config.get("audio.ducking.factor", 0.35) or 0.35),
    )
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
    session_store = None
    if config.get("session.auto_save", True):
        db = None
        if config.get("session.sqlite", True):
            db = SessionDatabase(config.config_dir / "sessions.db")
        session_store = SessionStore(sessions_dir=sessions_dir, db=db)
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
    pipeline_holder: dict = {"pipeline": build_pipeline(config)}

    def pipeline() -> Pipeline:
        return pipeline_holder["pipeline"]

    notes_llm = _make_llm(config, config.get("session.notes_llm") or config.get("llm.default"))
    notes_gen = NotesGenerator(llm=notes_llm)

    def wire_pipeline(p: Pipeline) -> None:
        p.on_segment = lambda seg: window.segment_received.emit(seg)
        p.on_result = lambda res: window.result_received.emit(res)
        p.on_asr_status = lambda msg: window.asr_status_received.emit(msg)
        p.on_state = lambda state: window.state_received.emit(state)

    wire_pipeline(pipeline())

    def on_record_toggled(checked: bool) -> None:
        if checked:
            if not ensure_recording_consent(config, parent=window):
                window.set_record_checked(False)
                return
            pipeline().start(
                loopback_device=config.get("transcribe.loopback.device"),
                mic_device=config.get("audio.mic_device"),
            )
        else:
            pipeline().stop()

    def on_notes_requested() -> None:
        window.set_notes_enabled(False)
        window.set_notes_status("生成中…")

        def worker() -> None:
            try:
                p = pipeline()
                terms = p.sessions.terms() if p.sessions else []
                notes = asyncio.run(notes_gen.generate(p.bus.context, terms=terms))
                path = p.sessions.append_notes(notes) if p.sessions is not None else None

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

    def on_settings() -> None:
        dlg = SettingsDialog(config, parent=window)
        if dlg.exec():
            # Rebuild pipeline so ASR/audio settings take effect next listen
            if pipeline().sessions and pipeline().sessions.active:
                QMessageBox.information(window, "设置", "请先停止监听，再开始以应用新设置。")
                return
            pipeline_holder["pipeline"] = build_pipeline(config)
            wire_pipeline(pipeline())
            threading.Thread(target=pipeline().warmup, daemon=True).start()
            QMessageBox.information(
                window,
                "设置",
                f"已保存。计算设备探测：{detect_compute_device().upper()}\n下次监听将使用新配置。",
            )

    def on_import() -> None:
        path, _ = QFileDialog.getOpenFileName(
            window,
            "导入音频",
            "",
            "Audio (*.wav *.mp3 *.flac *.m4a *.ogg);;All (*.*)",
        )
        if not path:
            return
        window.asr_status_received.emit("导入转写中…")

        def worker() -> None:
            try:
                audio, _sr = load_audio_file(path)
                # Prefer client (English) engine path for imported meeting audio
                ot = OfflineTranscriber(engine=pipeline().engine, chunk_seconds=3)
                # Dual engine routes by speaker; use client for EN-heavy imports
                text = ot.transcribe(audio, speaker="client")
                out_dir = config.config_dir / "sessions"
                out_dir.mkdir(parents=True, exist_ok=True)
                out = out_dir / f"import-{Path(path).stem}.md"
                out.write_text(
                    f"# 导入转写\n\n- source: {path}\n\n## 转写\n\n{text or '（无识别结果）'}\n",
                    encoding="utf-8",
                )

                def done() -> None:
                    window.asr_status_received.emit("ASR 就绪")
                    if text:
                        window.segment_received.emit(
                            TranscriptSegment(speaker="client", text=text[:2000], language="en")
                        )
                    QMessageBox.information(window, "导入完成", f"已保存:\n{out}")

                QTimer.singleShot(0, done)
            except Exception as exc:
                def fail() -> None:
                    window.asr_status_received.emit("ASR 就绪")
                    QMessageBox.warning(window, "导入失败", str(exc))

                QTimer.singleShot(0, fail)

        threading.Thread(target=worker, daemon=True).start()

    window._record_btn.toggled.connect(on_record_toggled)
    window.notes_requested.connect(on_notes_requested)
    def on_history() -> None:
        db = pipeline().sessions.db if pipeline().sessions else None
        if db is None:
            QMessageBox.information(window, "历史", "未启用 SQLite 会话库（session.sqlite）。")
            return
        HistoryDialog(db, parent=window).exec()

    window.settings_requested.connect(on_settings)
    window.import_requested.connect(on_import)
    window.history_requested.connect(on_history)

    window.show()

    if config.get("privacy.hide_from_screen_share", True):
        QTimer.singleShot(0, lambda: set_exclude_from_capture(window, True))

    threading.Thread(target=pipeline().warmup, daemon=True).start()

    sys.exit(app.exec())


if __name__ == "__main__":
    main()
