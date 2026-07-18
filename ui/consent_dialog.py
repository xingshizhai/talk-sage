from PySide6.QtWidgets import (
    QDialog,
    QVBoxLayout,
    QLabel,
    QDialogButtonBox,
    QTextBrowser,
    QWidget,
)
from config.manager import ConfigManager

_CONSENT_HTML = """
<p><b>录音与转写同意确认</b></p>
<p>TalkSage 会采集麦克风音频，并在启用系统回环时采集电脑扬声器输出（视频会议中的对方声音），
用于本地语音识别与会议辅助分析。</p>
<p>许多司法管辖区要求在录音前征得部分或全部参与者的同意
（例如美国部分州的双轨/全员同意、欧盟 GDPR 等）。</p>
<ul>
<li>您有责任确认在您所在地区录音是否合法，并在开始会话前取得所需同意。</li>
<li>开发者不对未经授权或不合法的录音承担任何责任。</li>
<li>默认情况下，语音识别在本地运行，音频不会上传；仅在启用相关插件时，
转写后的<strong>文本</strong>可能发送至您配置的 LLM 服务。</li>
</ul>
<p>点击「我已了解并同意」即表示您理解上述义务并愿意继续。</p>
"""


class RecordingConsentDialog(QDialog):
    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setWindowTitle("录音同意")
        self.setModal(True)
        self.setMinimumWidth(420)

        layout = QVBoxLayout(self)
        browser = QTextBrowser()
        browser.setOpenExternalLinks(False)
        browser.setHtml(_CONSENT_HTML)
        browser.setMinimumHeight(260)
        layout.addWidget(browser)

        buttons = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel
        )
        buttons.button(QDialogButtonBox.StandardButton.Ok).setText("我已了解并同意")
        buttons.button(QDialogButtonBox.StandardButton.Cancel).setText("取消")
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

        self.accept_button = buttons.button(QDialogButtonBox.StandardButton.Ok)
        self.reject_button = buttons.button(QDialogButtonBox.StandardButton.Cancel)


def ensure_recording_consent(config: ConfigManager, parent: QWidget | None = None) -> bool:
    """Return True if the user has consented (already or via dialog). Persists on accept."""
    if config.get("privacy.recording_consent_accepted"):
        return True
    dialog = RecordingConsentDialog(parent=parent)
    if dialog.exec() != QDialog.DialogCode.Accepted:
        return False
    config.set("privacy.recording_consent_accepted", True)
    config.save()
    return True
