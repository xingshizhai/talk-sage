import { useState, type CSSProperties, type ReactNode } from "react";
import type { LexiconDraft, PersonDraft } from "../lib/lexicon";

export default function LexiconManager({
  drafts,
  usedBy,
  onChange,
  onRemove,
  onAdd,
}: {
  drafts: LexiconDraft[];
  /** 词库 id → 正在使用它的场景名。 */
  usedBy: Record<string, string[]>;
  onChange: (id: string, patch: Partial<LexiconDraft>) => void;
  onRemove: (id: string) => void;
  onAdd: () => void;
}) {
  const [selectedId, setSelectedId] = useState(drafts[0]?.id ?? "");
  const [query, setQuery] = useState("");
  const selected = drafts.find((draft) => draft.id === selectedId) ?? drafts[0];
  const needle = query.trim().toLowerCase();
  const visible = drafts.filter((draft) => {
    if (!needle) return true;
    const haystack = [draft.label, draft.description, draft.terms, draft.corrections, ...draft.people.flatMap((person) => [person.name, person.aliases])]
      .join("\n")
      .toLowerCase();
    return haystack.includes(needle);
  });

  return (
    <div>
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8 }}>
        <h3 style={{ margin: 0, fontSize: 13 }}>词库</h3>
        <button type="button" onClick={onAdd} style={buttonStyle}>新建词库</button>
      </div>
      <div style={hintStyle}>
        词库存项目术语、常见误识别和参会人。在「场景模式」里选择某个场景是否加载、以及加载哪一份。开始监听时不再单独挑选。
      </div>
      {drafts.length === 0 ? (
        <div style={{ ...hintStyle, marginTop: 12 }}>还没有词库。新建一份后，再到场景模式里绑定。</div>
      ) : (
        <div style={{ display: "grid", gridTemplateColumns: "200px 1fr", gap: 12, marginTop: 12, alignItems: "start" }}>
          <div>
            <input
              aria-label="搜索词库"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索名称、术语、人名"
              style={{ ...inputStyle, width: "100%", marginBottom: 8 }}
            />
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {visible.map((draft) => {
                const active = draft.id === selected?.id;
                const terms = countLines(draft.terms);
                const scenes = usedBy[draft.id] ?? [];
                return (
                  <button
                    key={draft.id}
                    type="button"
                    onClick={() => setSelectedId(draft.id)}
                    style={{
                      textAlign: "left",
                      border: "1px solid var(--border)",
                      borderRadius: 8,
                      padding: "8px 10px",
                      cursor: "pointer",
                      background: active ? "var(--term-soft)" : "var(--surface-2)",
                      color: "var(--text)",
                    }}
                  >
                    <div style={{ fontSize: 12, fontWeight: 650 }}>{draft.label.trim() || "未命名词库"}</div>
                    <div style={{ fontSize: 10, color: "var(--muted)", marginTop: 3 }}>
                      {terms} 个术语 · {draft.people.length} 人
                      {scenes.length > 0 ? ` · ${scenes.join("、")}` : ""}
                    </div>
                  </button>
                );
              })}
              {visible.length === 0 && <div style={hintStyle}>没有匹配的词库。</div>}
            </div>
          </div>
          {selected && (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <div style={{ display: "flex", gap: 8 }}>
                <input
                  aria-label="词库名称"
                  value={selected.label}
                  onChange={(event) => onChange(selected.id, { label: event.target.value })}
                  placeholder="名称，例如周例会"
                  style={{ ...inputStyle, flex: 1 }}
                />
                <button type="button" onClick={() => onRemove(selected.id)} style={buttonStyle}>删除</button>
              </div>
              <input
                aria-label="词库说明"
                value={selected.description}
                onChange={(event) => onChange(selected.id, { description: event.target.value })}
                placeholder="说明这份词库用在什么场合"
                style={inputStyle}
              />
              <Field label={`术语 · ${countLines(selected.terms)}`}>
                <textarea
                  aria-label="词库术语"
                  value={selected.terms}
                  onChange={(event) => onChange(selected.id, { terms: event.target.value })}
                  placeholder={"每行一个项目名或术语，例如：\nTalkSage\n计费中台"}
                  rows={6}
                  style={areaStyle}
                />
              </Field>
              <Field label={`纠错 · ${countLines(selected.corrections)}`}>
                <textarea
                  aria-label="词库纠错"
                  value={selected.corrections}
                  onChange={(event) => onChange(selected.id, { corrections: event.target.value })}
                  placeholder={"误识别 => 正确写法，例如：\n拓思者 => TalkSage"}
                  rows={4}
                  style={areaStyle}
                />
              </Field>
              <Field label={`参会人 · ${selected.people.length}`}>
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  {selected.people.map((person, index) => (
                    <PersonRow
                      key={`${selected.id}-${index}`}
                      person={person}
                      onChange={(next) => onChange(selected.id, { people: replaceAt(selected.people, index, next) })}
                      onRemove={() => onChange(selected.id, { people: selected.people.filter((_, item) => item !== index) })}
                    />
                  ))}
                  <button
                    type="button"
                    onClick={() => onChange(selected.id, { people: [...selected.people, { name: "", aliases: "" }] })}
                    style={{ ...buttonStyle, alignSelf: "flex-start" }}
                  >
                    添加参会人
                  </button>
                  <div style={hintStyle}>显示名用于按人汇总。别名用 / 分开，识别到这些称呼时归到同一个人。</div>
                </div>
              </Field>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 11, color: "var(--muted)" }}>
      {label}
      {children}
    </label>
  );
}

function PersonRow({
  person,
  onChange,
  onRemove,
}: {
  person: PersonDraft;
  onChange: (person: PersonDraft) => void;
  onRemove: () => void;
}) {
  return (
    <div style={{ display: "flex", gap: 6 }}>
      <input
        aria-label="显示名"
        value={person.name}
        onChange={(event) => onChange({ ...person, name: event.target.value })}
        placeholder="显示名"
        style={{ ...inputStyle, width: 120 }}
      />
      <input
        aria-label="别名"
        value={person.aliases}
        onChange={(event) => onChange({ ...person, aliases: event.target.value })}
        placeholder="别名，用 / 分开"
        style={{ ...inputStyle, flex: 1 }}
      />
      <button type="button" onClick={onRemove} style={buttonStyle}>移除</button>
    </div>
  );
}

function replaceAt(people: PersonDraft[], index: number, next: PersonDraft): PersonDraft[] {
  return people.map((person, item) => item === index ? next : person);
}

function countLines(text: string): number {
  return text.split("\n").map((line) => line.trim()).filter(Boolean).length;
}

const inputStyle: CSSProperties = {
  font: "inherit",
  fontSize: 12,
  color: "var(--text)",
  background: "var(--surface-2)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "6px 8px",
};

const areaStyle: CSSProperties = { ...inputStyle, width: "100%", resize: "vertical" };

const buttonStyle: CSSProperties = {
  border: "1px solid var(--border)",
  background: "var(--surface-2)",
  color: "var(--text)",
  borderRadius: 6,
  padding: "6px 10px",
  cursor: "pointer",
  fontSize: 12,
};

const hintStyle: CSSProperties = { color: "var(--muted)", fontSize: 11, lineHeight: 1.6 };
