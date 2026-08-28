// 专业术语卡片的数据整形：一个事件可能带回多条术语，界面按条展示。

import type { TermItem } from "../sections/TermsSection";

/** 界面上的一条术语。 */
export interface TermRow {
  /** 卡片 key 与展开状态的键；同一事件里的第二条起加 `#i` 后缀。 */
  resultId: string;
  /** 术语本身（拆不出来时等于整行）。 */
  term: string;
  /** 解释；拆不出来时为空串。 */
  gloss: string;
  /** 整行原文（展开时显示）。 */
  raw: string;
  isFinal: boolean;
}

/**
 * 去重键：术语部分归一化（只保留字母数字与汉字，英文转小写）。
 *
 * 与 Rust 侧 `talksage_core::term_key` 同一套规则。监听中由事件出口统一去重，
 * 这里兜住没有会话的场景（未监听时手动查词直接推给界面，不经过那一层）。
 */
export function termKey(term: string): string {
  return [...term].filter((c) => /[\p{L}\p{N}]/u.test(c)).join("").toLowerCase();
}

/** 拆「术语：解释」；兼容旧格式 `NPI = 解释`。拆不出来时整行当术语。 */
function splitLine(line: string): { term: string; gloss: string } {
  const colon = line.indexOf("：");
  if (colon > 0) {
    return { term: line.slice(0, colon).trim(), gloss: line.slice(colon + 1).trim() };
  }
  const eq = line.indexOf(" = ");
  if (eq > 0) {
    return { term: line.slice(0, eq).trim(), gloss: line.slice(eq + " = ".length).trim() };
  }
  // 半角冒号：英文术语常写成 "SLA: service level agreement"
  const ascii = line.indexOf(": ");
  if (ascii > 0) {
    return { term: line.slice(0, ascii).trim(), gloss: line.slice(ascii + 1).trim() };
  }
  return { term: line, gloss: "" };
}

/**
 * 把术语事件摊平成一行一条。
 *
 * 一次提取最多给两条术语，插件把它们放在同一个事件里按行分隔 —— 直接整块渲染
 * 会挤成一张卡、计数也不对。
 */
export function toTermRows(items: TermItem[], dismissed: ReadonlySet<string> = new Set()): TermRow[] {
  const seen = new Set<string>();
  return items
    .flatMap((item) =>
    item.content
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line, i) => ({
        resultId: i === 0 ? item.resultId : `${item.resultId}#${i}`,
        ...splitLine(line),
        raw: line,
        isFinal: item.isFinal,
      })),
    )
    .filter((row) => {
      const key = termKey(row.term);
      if (!key) return true; // 取不出术语（如"识别中…"）：照常显示
      if (dismissed.has(key)) return false;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
}
