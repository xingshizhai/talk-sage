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

function sameCard(resultId: string, itemId: string): boolean {
  return resultId === itemId || resultId.startsWith(`${itemId}#`);
}

/** 当场讲解的后续行：并进上一张卡，不单独成条。 */
function isBriefingLine(term: string): boolean {
  return term === "关键" || term === "接话";
}

/**
 * 把专业知识事件摊成一张卡一条。
 *
 * 同一事件里用换行分开的「知识点：讲解」各成一张卡。「关键」「接话」以及没有冒号的续行
 * 并进上一张卡，避免被拆成没有标题的碎片。
 */
export function toTermRows(items: TermItem[], dismissed: ReadonlySet<string> = new Set()): TermRow[] {
  const seen = new Set<string>();
  const rows: TermRow[] = [];
  for (const item of items) {
    const lines = item.content.split("\n").map((line) => line.trim()).filter(Boolean);
    let part = 0;
    for (const line of lines) {
      const split = splitLine(line);
      const prev = rows[rows.length - 1];
      const continuation =
        prev &&
        sameCard(prev.resultId, item.resultId) &&
        (split.gloss === "" || isBriefingLine(split.term));
      if (continuation && prev) {
        prev.gloss = prev.gloss ? `${prev.gloss}\n${line}` : line;
        prev.raw = `${prev.raw}\n${line}`;
        continue;
      }
      const resultId = part === 0 ? item.resultId : `${item.resultId}#${part}`;
      part += 1;
      const row: TermRow = { resultId, ...split, raw: line, isFinal: item.isFinal };
      const key = termKey(row.term);
      if (!key) {
        rows.push(row);
        continue;
      }
      if (dismissed.has(key) || seen.has(key)) continue;
      seen.add(key);
      rows.push(row);
    }
  }
  return rows;
}
