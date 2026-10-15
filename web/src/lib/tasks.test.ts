import { describe, expect, it } from "vitest";
import type { KeyPoint } from "./highlights";
import { groupActionItems } from "./tasks";

function point(text: string, kind: KeyPoint["kind"], owner?: string): KeyPoint {
  return { resultId: text, kind, text, tsMs: 0, manual: false, owner, sourceRefs: [] };
}

describe("groupActionItems", () => {
  const people = [["张三", "张总"], ["李四"]];

  it("把别名收成花名册上的显示名，并按名册排序", () => {
    const groups = groupActionItems(
      [
        point("补测试", "行动", "张总"),
        point("写纪要", "行动", "李四"),
        point("这是决策", "决策", "张三"),
        point("对一下接口", "行动", "张三"),
      ],
      people,
    );
    expect(groups.map((group) => group.owner)).toEqual(["张三", "李四"]);
    expect(groups[0].items.map((item) => item.text)).toEqual(["补测试", "对一下接口"]);
  });

  it("没有负责人的行动项归到未指定", () => {
    const groups = groupActionItems([point("回头确认", "行动")], people);
    expect(groups).toEqual([{ owner: "未指定", items: [point("回头确认", "行动")] }]);
  });
});
