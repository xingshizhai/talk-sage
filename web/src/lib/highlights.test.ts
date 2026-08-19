import { describe, expect, it } from "vitest";
import { KeyPointAggregator, extractKeyPoint } from "./highlights";

describe("extractKeyPoint", () => {
  it("classifies questions", () => {
    const kp = extractKeyPoint("What is the MOQ for this product?", 1000);
    expect(kp?.kind).toBe("问句");
  });

  it("classifies decisions", () => {
    const kp = extractKeyPoint("我们决定采用方案 B", 1000);
    expect(kp?.kind).toBe("决策");
  });

  it("classifies requirements", () => {
    const kp = extractKeyPoint("We need NPI samples by Friday.", 1000);
    expect(kp?.kind).toBe("要求");
  });

  it("classifies technical", () => {
    const kp = extractKeyPoint("讨论一下接口的兼容性和并发性能", 1000);
    expect(kp?.kind).toBe("技术");
  });

  it("ignores short noise", () => {
    expect(extractKeyPoint("嗯", 1000)).toBeNull();
    expect(extractKeyPoint("好的", 1000)).toBeNull();
  });

  it("truncates long text", () => {
    const long = "A".repeat(200);
    const kp = extractKeyPoint(long, 1);
    expect(kp?.text.length).toBeLessThanOrEqual(121);
  });
});

describe("KeyPointAggregator", () => {
  it("dedupes identical consecutive points", () => {
    const agg = new KeyPointAggregator();
    agg.push("We need NPI samples", 1);
    agg.push("We need NPI samples", 2);
    expect(agg.getItems()).toHaveLength(1);
  });

  it("keeps distinct points", () => {
    const agg = new KeyPointAggregator();
    agg.push("What is the price?", 1);
    agg.push("We decided to proceed", 2);
    expect(agg.getItems()).toHaveLength(2);
  });
});
