import { describe, expect, it } from "vitest";
import { KeyPointAggregator, extractKeyPoint, textNoiseScore } from "./highlights";

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

  it("rejects noisy segments even if they match keywords", () => {
    // 重复/语气词密集的噪音文本即使含"技术"也不应聚合
    const noisy = extractKeyPoint("嗯嗯嗯对那个技术嗯嗯嗯", 1000);
    expect(noisy).toBeNull();
    const noisyAgg = new KeyPointAggregator();
    noisyAgg.push("嗯嗯嗯要求嗯嗯嗯嗯", 1);
    expect(noisyAgg.getItems()).toHaveLength(0);
  });

  it("truncates long text", () => {
    const long = "我们需要确认交期并讨论技术方案的兼容性性能延迟并发部署迁移规范。".repeat(8);
    const kp = extractKeyPoint(long, 1);
    expect(kp).not.toBeNull();
    expect(kp!.text.length).toBeLessThanOrEqual(121);
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
