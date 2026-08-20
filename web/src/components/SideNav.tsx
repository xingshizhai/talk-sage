// 左侧导航栏：品牌 logo + 导航项 + 运行状态 + 监听/调试按钮。

import type { Theme } from "../lib/theme";
import talksageMark from "../assets/talksage-mark.svg";

export interface NavItem {
  key: string;
  label: string;
  dot: string;
  badge: string;
  active: boolean;
}

export interface HealthRow {
  dot: string;
  label: string;
  value: string;
}

export default function SideNav({
  theme,
  onToggleTheme,
  navItems,
  healthRows,
  listening,
  onToggleListen,
  onOpenDebug,
  onNavigate,
  noiseLevel,
  onNoiseLevel,
  micRms,
}: {
  theme: Theme;
  onToggleTheme: () => void;
  navItems: NavItem[];
  healthRows: HealthRow[];
  listening: boolean;
  onToggleListen: () => void;
  onOpenDebug: () => void;
  onNavigate: (key: string) => void;
  noiseLevel: number;
  onNoiseLevel: (level: number) => void;
  micRms: number;
}) {
  return (
    <nav
      style={{
        width: 210,
        borderRight: "1px solid var(--border)",
        background: "var(--surface)",
        display: "flex",
        flexDirection: "column",
        overflowY: "auto",
      }}
    >
      {/* 品牌 */}
      <div style={{ padding: "14px 14px 8px", display: "flex", alignItems: "center", gap: 8 }}>
        <img src={talksageMark} alt="拓思者 logo" style={{ width: 22, height: 22, borderRadius: 5, flexShrink: 0 }} />
        <b style={{ fontSize: 14 }}>拓思者</b>
        <span style={{ fontSize: 9, color: "var(--muted)", fontFamily: "monospace" }}>TalkSage</span>
        <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>
          {theme === "dark" ? "深色" : "浅色"}
        </span>
        <button
          onClick={onToggleTheme}
          style={{ fontSize: 11, padding: "2px 8px", borderRadius: 6, cursor: "pointer", background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--border)" }}
        >
          {theme === "dark" ? "☀" : "☾"}
        </button>
      </div>

      {/* 导航项 */}
      <div style={{ display: "flex", flexDirection: "column", gap: 2, padding: "6px 10px" }}>
        {navItems.map((n) => (
          <button
            key={n.key}
            onClick={() => onNavigate(n.key)}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 10,
              width: "100%",
              textAlign: "left",
              padding: "9px 10px",
              border: "none",
              borderRadius: 9,
              cursor: "pointer",
              font: "inherit",
              fontSize: 13,
              fontWeight: 600,
              background: n.active ? "var(--surface-2)" : "transparent",
              color: n.active ? "var(--text)" : "var(--text-2)",
            }}
          >
            <span style={{ width: 6, height: 6, borderRadius: 2, background: n.dot }} />
            {n.label}
            <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{n.badge}</span>
          </button>
        ))}
      </div>

      <div style={{ flex: 1 }} />

      {/* 运行状态 */}
      <div style={{ margin: "0 10px 10px", padding: "10px 11px", borderRadius: 10, background: "var(--surface-2)", border: "1px solid var(--border)" }}>
        <div style={{ fontSize: 10, letterSpacing: "0.08em", color: "var(--muted)", fontWeight: 700, marginBottom: 7 }}>运行状态</div>
        {healthRows.map((h, i) => (
          <div key={i} style={{ display: "flex", alignItems: "center", gap: 7, fontSize: 11, fontFamily: "monospace", color: "var(--text-2)", padding: "2px 0" }}>
            <span style={{ width: 5, height: 5, borderRadius: "50%", background: h.dot }} />
            <span>{h.label}</span>
            <span style={{ marginLeft: "auto", color: "var(--text)" }}>{h.value}</span>
          </div>
        ))}
      </div>

      {/* 音频控制（麦克风电平 + 噪音电平阈值，监听中实时可用） */}
      {listening && (
        <div style={{ margin: "0 10px 10px", padding: "10px 11px", borderRadius: 10, background: "var(--surface-2)", border: "1px solid var(--border)" }}>
          <div style={{ fontSize: 10, letterSpacing: "0.08em", color: "var(--muted)", fontWeight: 700, marginBottom: 7 }}>音频控制</div>

          {/* 麦克风电平 */}
          <div style={{ marginBottom: 9 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 4 }}>
              <span style={{ fontSize: 10, letterSpacing: "0.08em", color: "var(--muted)", fontWeight: 700 }}>麦克风电平</span>
              <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>
                {Math.round(Math.min(1, micRms * 10) * 100)}%
              </span>
            </div>
            <div style={{ height: 6, borderRadius: 3, background: "var(--border)", overflow: "hidden" }}>
              <div
                style={{
                  height: "100%",
                  width: `${Math.min(100, micRms * 1000)}%`,
                  borderRadius: 3,
                  background:
                    micRms < 0.05
                      ? "var(--live)"
                      : micRms < 0.2
                        ? "var(--brief)"
                        : "var(--danger)",
                  transition: "width 80ms linear",
                }}
              />
            </div>
            <div style={{ marginTop: 3, fontSize: 9, color: "var(--muted)", fontFamily: "monospace" }}>
              {micRms < 0.003 ? "无声" : micRms < 0.05 ? "微弱" : micRms < 0.2 ? "正常" : "过载"}
            </div>
          </div>

          {/* 分隔线 */}
          <div style={{ borderTop: "1px solid var(--border)", margin: "2px 0 9px" }} />

          {/* 噪音电平阈值 */}
          <div>
            <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 4 }}>
              <span style={{ fontSize: 10, letterSpacing: "0.08em", color: "var(--muted)", fontWeight: 700 }}>噪音电平阈值</span>
              <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--live)", fontFamily: "monospace", fontWeight: 600 }}>
                {noiseLevel}%
              </span>
            </div>
            <input
              type="range"
              min={0}
              max={100}
              step={1}
              value={noiseLevel}
              onChange={(e) => onNoiseLevel(Number(e.target.value))}
              title="实时调节噪音电平阈值：调高抑制背景噪音（轻声也可能被压），调低更灵敏"
              style={{ width: "100%", accentColor: "var(--live)", cursor: "pointer" }}
            />
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: 10, color: "var(--muted)", fontFamily: "monospace", marginTop: 2 }}>
              <span>0 关闭</span>
              <span>{noiseLevel === 0 ? "默认" : noiseLevel < 50 ? "弱抑制" : "强抑制"}</span>
            </div>
          </div>
        </div>
      )}

      {/* 监听 / 调试 */}
      <div style={{ margin: "0 10px 12px", display: "flex", flexDirection: "column", gap: 6 }}>
        <button
          onClick={onToggleListen}
          style={{
            padding: "9px 0",
            borderRadius: 9,
            border: "none",
            fontWeight: 600,
            cursor: "pointer",
            background: listening ? "var(--danger)" : "var(--live)",
            color: "#fff",
          }}
        >
          {listening ? "⏹ 停止监听" : "▶ 开始监听"}
        </button>
        <button
          onClick={onOpenDebug}
          style={{ padding: "8px 0", borderRadius: 9, cursor: "pointer", background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--border)" }}
        >
          调试
        </button>
      </div>
    </nav>
  );
}
