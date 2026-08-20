#!/usr/bin/env python3
"""Generate the architecture diagram for the 拓思者 (TalkSage) README / docs.

Style modeled after WhisperLiveKit's generate_architecture.py.
Output: docs/architecture.png
"""

import os

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.font_manager as fm
from matplotlib.patches import FancyBboxPatch

# ── CJK 字体（跨平台探测：微软雅黑 / 黑体 / PingFang / Noto）──
_CJK_FONTS = [
    "C:/Windows/Fonts/msyh.ttc",                 # Windows 微软雅黑
    "C:/Windows/Fonts/simhei.ttf",               # Windows 黑体
    "/System/Library/Fonts/PingFang.ttc",        # macOS 苹方
    "/System/Library/Fonts/STHeiti Light.ttc",   # macOS 黑体
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",  # Linux Noto
]
for _f in _CJK_FONTS:
    if os.path.exists(_f):
        try:
            fm.fontManager.addfont(_f)
        except Exception:
            pass
plt.rcParams["font.family"] = "sans-serif"
plt.rcParams["font.sans-serif"] = ["Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", "SimHei", "DejaVu Sans"]
plt.rcParams["axes.unicode_minus"] = False

# ── Colours ──
C_BG      = "#101013"
C_PANEL   = "#17171b"
C_PANEL2  = "#1e1e23"
C_BORDER  = "#2a2a31"
C_TEXT    = "#ececea"
C_TEXTDIM = "#7e7e86"
C_ME      = "#8ea4f0"   # 我
C_CLIENT  = "#6fc8bf"   # 客户
C_TERM    = "#b28fe8"   # 术语
C_BRIEF   = "#d8c06a"   # 简报
C_LIVE    = "#6fe0a0"   # 活跃/监听
C_DANGER  = "#ef7a70"
C_BOX_BG  = "#1a1a20"
C_BOX_BG2 = "#221a2e"
C_BOX_BG3 = "#1a2a22"

fig, ax = plt.subplots(1, 1, figsize=(20, 12), facecolor=C_BG)
ax.set_xlim(0, 20)
ax.set_ylim(0, 12)
ax.set_aspect("equal")
ax.axis("off")
fig.subplots_adjust(left=0.01, right=0.99, top=0.97, bottom=0.01)


def box(x, y, w, h, label, color=C_BORDER, bg=C_BOX_BG, fontsize=7, bold=False,
        text_color=C_TEXT, radius=0.12):
    rect = FancyBboxPatch((x, y), w, h, boxstyle=f"round,pad=0.05,rounding_size={radius}",
                          facecolor=bg, edgecolor=color, linewidth=1.2)
    ax.add_patch(rect)
    ax.text(x + w / 2, y + h / 2, label, ha="center", va="center",
            fontsize=fontsize, color=text_color, fontweight="bold" if bold else "normal",
            family=["DejaVu Sans Mono", "Microsoft YaHei"])
    return rect


def arrow(x1, y1, x2, y2, color=C_TEXTDIM, style="->", lw=1.4):
    ax.annotate("", xy=(x2, y2), xytext=(x1, y1),
                arrowprops=dict(arrowstyle=style, color=color, lw=lw))


def section_box(x, y, w, h, title, bg=C_PANEL, border=C_BORDER, title_color=C_ME):
    rect = FancyBboxPatch((x, y), w, h, boxstyle="round,pad=0.05,rounding_size=0.2",
                          facecolor=bg, edgecolor=border, linewidth=1.5)
    ax.add_patch(rect)
    ax.text(x + 0.15, y + h - 0.25, title, ha="left", va="top",
            fontsize=9, color=title_color, fontweight="bold", family=["DejaVu Sans Mono", "Microsoft YaHei"])


# ═══════════ Title ═══════════
ax.text(10, 11.7, "拓思者 (TalkSage) Architecture — AI 会议助理", ha="center", va="center",
        fontsize=15, color=C_TEXT, fontweight="bold", family="sans-serif")
ax.text(10, 11.3, "本地实时转写 · 说话人识别 · 会议智能分析 · 录音闭环 · 双载体（Tauri / headless）",
        ha="center", va="center", fontsize=7, color=C_TEXTDIM, family="sans-serif")

# ═══════════ Left: Clients ═══════════
section_box(0.1, 6.2, 3.6, 4.6, "Clients / UI（React）", border=C_ME)
box(0.3, 9.9, 1.6, 0.5, "Tauri 桌面\n(IPC)", color=C_ME, fontsize=6.5, bold=True)
box(2.1, 9.9, 1.4, 0.5, "Headless\n(WS/HTTP)", color=C_ME, fontsize=6.5)
box(0.3, 8.9, 3.2, 0.5, "实时转写 · 历史(回放) · 设置", color=C_ME, fontsize=6.5)
box(0.3, 8.1, 3.2, 0.5, "要点聚合 · 术语 · 简报", color=C_TERM, fontsize=6.5)
box(0.3, 7.3, 3.2, 0.5, "麦克风电平 · 噪音电平阈值 · 声音标识", color=C_CLIENT, fontsize=6.5)
box(0.3, 6.5, 3.2, 0.5, "系统托盘 / 菜单栏（窗口恢复）", color=C_BRIEF, fontsize=6.5)

# ═══════════ Centre: Adapter / Event bus ═══════════
section_box(4.0, 5.0, 3.9, 5.8, "Adapter / DomainEvent 总线", border=C_CLIENT, bg=C_PANEL2)
box(4.3, 9.9, 3.3, 0.6, "Commands: start/stop · set_noise_level\n声纹 · 会话 · 回放", color=C_CLIENT, fontsize=6, bold=True)
box(4.3, 9.0, 3.3, 0.6, "DomainEvent（serde tag）\nsegment/term/translation/key_point\nbrief/status/level/session_stats", color=C_TERM, fontsize=5.5)
box(4.3, 7.4, 3.3, 1.0, "RuntimeParams（实时可调）\n噪音电平阈值 → pipeline", color=C_BRIEF, fontsize=6)
box(4.3, 6.2, 3.3, 0.8, "SessionStore（SQLite）\n段+统计+质量 meta", color=C_LIVE, fontsize=6)
box(4.3, 5.4, 3.3, 0.5, "Recordings（wav 回放）", color=C_LIVE, fontsize=6)

# ═══════════ Right: Pipeline ═══════════
section_box(8.2, 4.6, 11.6, 6.8, "talksage-pipeline（实时管道 · 每流独立）", border=C_LIVE, bg="#10181a")

# 上行：音频链路
box(8.5, 9.8, 2.0, 0.8, "AudioHub\nmic (cpal) / loopback", color=C_LIVE, bg=C_BOX_BG3, fontsize=6.5, bold=True)
box(10.9, 9.8, 2.0, 0.8, "Preprocessor\n高通 · 噪声门", color=C_LIVE, bg=C_BOX_BG3, fontsize=6.5)
box(13.3, 9.8, 2.0, 0.8, "VAD\nsilero 分段", color=C_LIVE, bg=C_BOX_BG3, fontsize=6.5)
box(15.7, 9.8, 2.4, 0.8, "Streaming ASR\nsherpa · 引擎池常驻", color=C_ME, bg=C_BOX_BG, fontsize=6.5, bold=True)
arrow(10.5, 10.2, 10.9, 10.2, color=C_TEXTDIM)
arrow(12.9, 10.2, 13.3, 10.2, color=C_TEXTDIM)
arrow(15.3, 10.2, 15.7, 10.2, color=C_TEXTDIM)

# 中行：段后分析
box(8.5, 8.2, 2.2, 1.0, "段统计\n时长/RMS/Level", color=C_TEXTDIM, fontsize=6)
box(11.0, 8.2, 2.4, 1.0, "说话人识别\nwespeaker 声纹 · 聚类", color=C_TERM, bg=C_BOX_BG2, fontsize=6, bold=True)
box(13.7, 8.2, 2.0, 1.0, "Plugins\n术语/翻译/简报", color=C_BRIEF, bg="#241f12", fontsize=6)
box(16.0, 8.2, 1.5, 1.0, "录音器\n原始 PCM", color=C_LIVE, fontsize=6)
arrow(9.6, 9.8, 9.6, 9.2, color=C_TEXTDIM)
arrow(12.3, 9.8, 12.3, 9.2, color=C_TEXTDIM)
arrow(14.7, 9.8, 14.7, 9.2, color=C_TEXTDIM)
arrow(17.0, 9.8, 17.0, 9.2, color=C_TEXTDIM)

# 下行：事件输出
box(8.5, 6.6, 3.6, 1.0, "Segment final（增量 partial→final）\n→ DomainEvent", color=C_TEXT, bg=C_BOX_BG, fontsize=6, bold=True)
box(12.4, 6.6, 3.6, 1.0, "会话结束 SessionStats →\n质量评估（噪音/静音判定）", color=C_BRIEF, bg="#241f12", fontsize=6)
box(16.3, 6.6, 1.3, 1.0, "Level\n事件", color=C_CLIENT, fontsize=6)

# 底部共享组件
section_box(8.2, 0.5, 11.6, 3.8, "Shared Components（跨会话共享）", border=C_TEXTDIM, bg=C_PANEL2)
box(8.5, 2.8, 3.4, 1.0, "EnginePool\nASR 引擎常驻（热启动）", color=C_ME, fontsize=6.5, bold=True)
box(12.2, 2.8, 3.4, 1.0, "SpeakerIdentifier\nwespeaker 声纹共享", color=C_TERM, fontsize=6.5)
box(15.9, 2.8, 3.6, 1.0, "PluginContext\nLLM（OpenAI 兼容）· 知识库", color=C_BRIEF, fontsize=6.5)
box(8.5, 1.4, 3.4, 1.0, "SessionStore\nSQLite 会话/历史", color=C_LIVE, fontsize=6.5)
box(12.2, 1.4, 3.4, 1.0, "QualityParams\n噪音阈值（可配置/自动检测）", color=C_BRIEF, fontsize=6.5)
box(15.9, 1.4, 3.6, 1.0, "RuntimeParams\n噪音电平阈值（实时）", color=C_CLIENT, fontsize=6.5)

# ═══════════ Arrows: main flow ═══════════
# Client → Adapter
arrow(3.7, 9.0, 4.0, 9.0, color=C_ME, lw=2)
ax.text(3.75, 9.25, "invoke / WS", fontsize=5.5, color=C_ME, family=["DejaVu Sans Mono", "Microsoft YaHei"])
# Adapter → Pipeline
arrow(7.9, 9.0, 8.2, 9.0, color=C_LIVE, lw=2)
ax.text(7.92, 9.3, "start_listen / cfg", fontsize=5.5, color=C_LIVE, family=["DejaVu Sans Mono", "Microsoft YaHei"])
# Pipeline → Adapter (events)
arrow(8.2, 7.2, 7.9, 7.2, color=C_TERM, lw=2)
ax.text(7.92, 7.5, "DomainEvent", fontsize=5.5, color=C_TERM, family=["DejaVu Sans Mono", "Microsoft YaHei"])
# Adapter → Client (events)
arrow(4.0, 7.2, 3.7, 7.2, color=C_TERM, lw=2)
# RuntimeParams → pipeline
arrow(6.0, 7.4, 6.0, 4.8, color=C_BRIEF, lw=1.4, style="<->")

# Legend
ax.text(0.3, 5.2, "数据流:", fontsize=7, color=C_TEXT, fontweight="bold", family=["DejaVu Sans Mono", "Microsoft YaHei"])
for i, (label, color) in enumerate([
    ("控制（invoke / WS / start_listen）", C_ME),
    ("音频（PCM → VAD → ASR）", C_LIVE),
    ("事件（DomainEvent 推送）", C_TERM),
    ("运行期调节（RuntimeParams）", C_BRIEF),
]):
    ax.plot([0.35], [4.7 - i * 0.3], "s", color=color, markersize=6)
    ax.text(0.55, 4.7 - i * 0.3, label, fontsize=6, color=color, va="center", family=["DejaVu Sans Mono", "Microsoft YaHei"])

plt.savefig("docs/architecture.png", dpi=200, facecolor=C_BG, bbox_inches="tight", pad_inches=0.1)
print("Saved docs/architecture.png")
