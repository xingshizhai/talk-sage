import type { LexiconPack, SceneMode } from "./api";

export interface PersonDraft {
  name: string;
  aliases: string;
}

export interface LexiconDraft {
  id: string;
  label: string;
  description: string;
  terms: string;
  corrections: string;
  people: PersonDraft[];
}

export function lexiconIdForScene(
  lexiconIds: Partial<Record<SceneMode, string>> | undefined,
  mode: SceneMode | undefined,
  packs: Record<string, unknown> | undefined,
): string | null {
  if (!mode || !lexiconIds) return null;
  const id = lexiconIds[mode];
  if (!id || !packs?.[id]) return null;
  return id;
}

export function lexiconDraftsFromConfig(lexicons: Record<string, LexiconPack> | undefined): LexiconDraft[] {
  return Object.entries(lexicons ?? {}).map(([id, pack]) => ({
    id,
    label: pack.label ?? "",
    description: pack.description ?? "",
    terms: (pack.terms ?? []).join("\n"),
    corrections: formatCorrectionLines(pack.corrections ?? {}),
    people: (pack.people ?? []).map((group) => ({
      name: group[0] ?? "",
      aliases: group.slice(1).join(" / "),
    })),
  }));
}

export function lexiconDraftToPack(draft: LexiconDraft): LexiconPack {
  const label = draft.label.trim();
  return {
    label: label || draft.id,
    description: draft.description.trim(),
    terms: draft.terms.split("\n").map((value) => value.trim()).filter(Boolean),
    corrections: parseCorrectionLines(draft.corrections),
    people: draft.people.flatMap((person) => {
      const aliases = person.aliases.split(/[/、,，]/).map((part) => part.trim()).filter(Boolean);
      const name = person.name.trim() || aliases.shift() || "";
      if (!name) return [];
      return [[name, ...aliases.filter((alias) => alias !== name)]];
    }),
  };
}

export function newLexiconId(existing: readonly string[]): string {
  let n = existing.length + 1;
  let id = `lexicon-${n}`;
  while (existing.includes(id)) {
    n += 1;
    id = `lexicon-${n}`;
  }
  return id;
}

export function parseCorrectionLines(text: string): Record<string, string> {
  const corrections: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const [wrong, ...right] = line.split("=>");
    const from = wrong?.trim() ?? "";
    const to = right.join("=>").trim();
    if (from && to) corrections[from] = to;
  }
  return corrections;
}

function formatCorrectionLines(corrections: Record<string, string>): string {
  return Object.entries(corrections).map(([wrong, right]) => `${wrong} => ${right}`).join("\n");
}
