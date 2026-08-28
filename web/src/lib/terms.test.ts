import { describe, expect, it } from "vitest";
import { termKey, toTermRows } from "./terms";

describe("toTermRows", () => {
  it("把一个事件里的多条术语摊平成一行一条", () => {
    const rows = toTermRows([
      {
        resultId: "term-1",
        content: "MOQ：最小起订量，供应商单次接单的最低数量门槛。\n灰度发布：新版本先放小比例用户。",
        isFinal: true,
      },
    ]);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ resultId: "term-1", term: "MOQ" });
    expect(rows[0].gloss).toBe("最小起订量，供应商单次接单的最低数量门槛。");
    // 第二条要有自己的 key，否则展开状态会串
    expect(rows[1]).toMatchObject({ resultId: "term-1#1", term: "灰度发布" });
  });

  it("兼容旧的 “NPI = 解释” 格式", () => {
    const rows = toTermRows([{ resultId: "t", content: "NPI = New Product Introduction，新产品导入", isFinal: true }]);
    expect(rows[0].term).toBe("NPI");
    expect(rows[0].gloss).toBe("New Product Introduction，新产品导入");
  });

  it("拆不出分隔符时整行当作术语，不丢内容", () => {
    const rows = toTermRows([{ resultId: "t", content: "专业术语识别中…", isFinal: false }]);
    expect(rows).toHaveLength(1);
    expect(rows[0].term).toBe("专业术语识别中…");
    expect(rows[0].gloss).toBe("");
    expect(rows[0].isFinal).toBe(false);
  });

  it("同一个术语只显示一条（自动提取与手动查词都算）", () => {
    const rows = toTermRows([
      { resultId: "auto", content: "付鹏：经济学家，以直白敢言著称", isFinal: true },
      { resultId: "kw", content: "付鹏：指东北证券首席经济学家", isFinal: true },
      { resultId: "manual", content: "MOQ：最小起订量", isFinal: true },
      { resultId: "manual2", content: "moq: minimum order quantity", isFinal: true },
    ]);
    expect(rows.map((r) => r.resultId)).toEqual(["auto", "manual"]);
  });

  it("空内容不产生卡片（撤销骨架的事件）", () => {
    expect(toTermRows([{ resultId: "t", content: "", isFinal: true }])).toHaveLength(0);
    expect(toTermRows([{ resultId: "t", content: "  \n  ", isFinal: true }])).toHaveLength(0);
  });

  it("删除后按归一化术语键持续屏蔽后续结果", () => {
    const dismissed = new Set([termKey("MOQ")]);
    const rows = toTermRows(
      [
        { resultId: "old", content: "MOQ：最小起订量\nSLA：服务等级协议", isFinal: true },
        { resultId: "new-summary", content: "moq: minimum order quantity", isFinal: true },
      ],
      dismissed,
    );
    expect(rows.map((row) => row.term)).toEqual(["SLA"]);
  });
});
