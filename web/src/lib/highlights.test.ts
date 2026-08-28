import { describe, expect, it } from "vitest";
import { categoryLabel, keyPointKey, textNoiseScore, toKeyPoint } from "./highlights";

describe("keyPointKey", () => {
  it("忽略标点、空白和英文大小写", () => {
    expect(keyPointKey("下周完成 API 文档。")).toBe(keyPointKey("下周完成 api文档"));
  });
});

describe("textNoiseScore", () => {
  it("scores clean speech low", () => {
    expect(textNoiseScore("我们需要在周五之前拿到 NPI 样品")).toBeLessThan(0.3);
  });

  it("scores repeated filler noise high", () => {
    expect(textNoiseScore("嗯嗯嗯嗯对嗯嗯嗯")).toBeGreaterThan(0.45);
    expect(textNoiseScore("嗯嗯嗯嗯嗯嗯嗯嗯")).toBeGreaterThan(0.7);
    expect(textNoiseScore("那个个那个个那个个那个个")).toBeGreaterThan(0.45);
  });
});

describe("categoryLabel", () => {
  it("maps backend snake_case categories", () => {
    expect(categoryLabel("question")).toBe("问句");
    expect(categoryLabel("requirement")).toBe("要求");
    expect(categoryLabel("decision")).toBe("决策");
    expect(categoryLabel("action")).toBe("行动");
    expect(categoryLabel("technical")).toBe("技术");
    expect(categoryLabel("other")).toBe("其他");
  });

  it("falls back to 其他 for unknown values", () => {
    expect(categoryLabel("unknown")).toBe("其他");
  });
});

describe("toKeyPoint", () => {
  it("copies identity and maps category", () => {
    const kp = toKeyPoint({
      result_id: "kp-1",
      category: "requirement",
      content: "We need NPI samples by Friday.",
      ts_ms: 42,
    });
    expect(kp).toEqual({
      resultId: "kp-1",
      kind: "要求",
      text: "We need NPI samples by Friday.",
      tsMs: 42,
      manual: false,
      sourceRefs: [],
    });
  });

  it("保留行动项负责人、截止时间和来源引用", () => {
    expect(toKeyPoint({ result_id: "a", category: "action", content: "更新文档", owner: "张三", due_date: "周五", source_refs: [2, 3] }))
      .toMatchObject({ owner: "张三", dueDate: "周五", sourceRefs: [2, 3] });
  });
});
