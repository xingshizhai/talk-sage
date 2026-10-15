import type { KeyPoint } from "../lib/highlights";
import { groupActionItems } from "../lib/tasks";

export default function TaskBoard({
  points,
  people,
}: {
  points: readonly KeyPoint[];
  people: readonly (readonly string[])[];
}) {
  const groups = groupActionItems(points, people);
  return (
    <section
      style={{
        background: "var(--card-bg)",
        border: "var(--card-border)",
        borderRadius: "var(--card-radius)",
        boxShadow: "var(--card-shadow)",
        padding: "var(--pad)",
      }}
    >
      <div style={{ fontSize: 13, fontWeight: 650, marginBottom: 8 }}>按人汇总的任务</div>
      {groups.length === 0 ? (
        <div style={{ fontSize: 12, color: "var(--muted)" }}>安排到人的任务会汇总在这里。</div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          {groups.map((group) => (
            <div key={group.owner}>
              <div style={{ fontSize: 12, fontWeight: 650, marginBottom: 4 }}>{group.owner}</div>
              {group.items.map((item) => (
                <div key={item.resultId} style={{ fontSize: 13, lineHeight: 1.55, color: "var(--text)" }}>
                  {item.text}
                  {item.dueDate ? <span style={{ marginLeft: 8, fontSize: 11, color: "var(--muted)" }}>{item.dueDate}</span> : null}
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
