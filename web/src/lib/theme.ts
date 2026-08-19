// 界面主题：浅色/深色 CSS 变量（语义色：me/client/term/brief/live）。

export type Theme = "light" | "dark";

export interface ThemeVars {
  bg: string;
  surface: string;
  surface2: string;
  border: string;
  text: string;
  text2: string;
  muted: string;
  me: string;
  meSoft: string;
  client: string;
  clientSoft: string;
  term: string;
  termSoft: string;
  brief: string;
  briefSoft: string;
  live: string;
  liveSoft: string;
  danger: string;
}

export const THEMES: Record<Theme, ThemeVars> = {
  light: {
    bg: "#f2f2f1",
    surface: "#ffffff",
    surface2: "#f7f7f5",
    border: "#e4e4e1",
    text: "#1b1b19",
    text2: "#3f3f3b",
    muted: "#86867e",
    me: "#3b5bd6",
    meSoft: "#eceffc",
    client: "#2e8f87",
    clientSoft: "#e8f5f3",
    term: "#7b4fd0",
    termSoft: "#f2edfb",
    brief: "#9a7b1f",
    briefSoft: "#f7f2e0",
    live: "#1f9d55",
    liveSoft: "#e5f6ec",
    danger: "#c4342a",
  },
  dark: {
    bg: "#101013",
    surface: "#17171b",
    surface2: "#1e1e23",
    border: "#2a2a31",
    text: "#ececea",
    text2: "#b9b9c0",
    muted: "#7e7e86",
    me: "#8ea4f0",
    meSoft: "rgba(142,164,240,0.14)",
    client: "#6fc8bf",
    clientSoft: "rgba(111,200,191,0.12)",
    term: "#b28fe8",
    termSoft: "rgba(178,143,232,0.14)",
    brief: "#d8c06a",
    briefSoft: "rgba(216,192,106,0.12)",
    live: "#6fe0a0",
    liveSoft: "rgba(111,224,160,0.12)",
    danger: "#ef7a70",
  },
};

export function cssVars(t: Theme): Record<string, string> {
  const v = THEMES[t];
  return {
    "--bg": v.bg,
    "--surface": v.surface,
    "--surface-2": v.surface2,
    "--border": v.border,
    "--text": v.text,
    "--text-2": v.text2,
    "--muted": v.muted,
    "--me": v.me,
    "--me-soft": v.meSoft,
    "--client": v.client,
    "--client-soft": v.clientSoft,
    "--term": v.term,
    "--term-soft": v.termSoft,
    "--brief": v.brief,
    "--brief-soft": v.briefSoft,
    "--live": v.live,
    "--live-soft": v.liveSoft,
    "--danger": v.danger,
    "--pad": "18px",
    "--fs-sm": "13px",
    "--gap": "12px",
    "--line": "1.6",
  };
}
