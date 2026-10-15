import type { KeyPoint } from "./highlights";

export interface TaskGroup {
  owner: string;
  items: KeyPoint[];
}

function normalizeName(value: string): string {
  return value.replace(/\s+/g, "").toLowerCase();
}

/** 用花名册把「张总」收成显示名。对不上的称呼原样保留。 */
export function canonicalOwner(owner: string, people: readonly (readonly string[])[]): string {
  const trimmed = owner.trim();
  const key = normalizeName(trimmed);
  if (!key) return "";
  for (const group of people) {
    const display = group[0]?.trim();
    if (!display) continue;
    if (group.some((alias) => normalizeName(alias) === key)) return display;
  }
  return trimmed;
}

/** 只汇总行动项。花名册里的人按名册顺序，其余按名字，未指定放最后。 */
export function groupActionItems(points: readonly KeyPoint[], people: readonly (readonly string[])[]): TaskGroup[] {
  const buckets = new Map<string, KeyPoint[]>();
  for (const point of points) {
    if (point.kind !== "行动") continue;
    const owner = canonicalOwner(point.owner ?? "", people) || "未指定";
    const list = buckets.get(owner);
    if (list) list.push(point);
    else buckets.set(owner, [point]);
  }

  const ordered: TaskGroup[] = [];
  const used = new Set<string>();
  for (const group of people) {
    const display = group[0]?.trim();
    if (!display || used.has(display) || !buckets.has(display)) continue;
    used.add(display);
    ordered.push({ owner: display, items: buckets.get(display) ?? [] });
  }
  const rest = [...buckets.keys()]
    .filter((owner) => owner !== "未指定" && !used.has(owner))
    .sort((a, b) => a.localeCompare(b, "zh"));
  for (const owner of rest) ordered.push({ owner, items: buckets.get(owner) ?? [] });
  const unassigned = buckets.get("未指定");
  if (unassigned) ordered.push({ owner: "未指定", items: unassigned });
  return ordered;
}
