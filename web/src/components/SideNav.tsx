// 左侧导航栏：导航项 + 运行状态 + 监听/调试按钮。

import type { Theme } from "../lib/theme";

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
}: {
  theme: Theme;
  onToggleTheme: () => void;
  navItems: NavItem[];
  healthRows: HealthRow[];
  listening: boolean;
  onToggleListen: () => void;
  onOpenDebug: () => void;
  onNavigate: (key: string) => void;
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
        <span style={{ width: 9, height: 9, borderRadius: 3, background: "var(--live)" }} />
        <b style={{ fontSize: 14 }}>TalkSage</b>
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
