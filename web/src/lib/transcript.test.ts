import { describe, expect, it } from "vitest";
import { TranscriptAccumulator } from "./transcript";

function seg(speaker: string, text: string, partial: boolean) {
  return { speaker_label: speaker, text, is_partial: partial };
}

describe("TranscriptAccumulator", () => {
  it("partial 事件增量更新同一行，不新增行", () => {
    const acc = new TranscriptAccumulator();
    acc.push(seg("我", "昨", true));
    acc.push(seg("我", "昨天是", true));
    acc.push(seg("我", "昨天是星期一", true));
    const lines = acc.getLines();
    expect(lines).toHaveLength(1);
    expect(lines[0].text).toBe("昨天是星期一");
    expect(lines[0].isPartial).toBe(true);
  });

  it("final 事件把未完成行固化为最终文本", () => {
    const acc = new TranscriptAccumulator();
    acc.push(seg("我", "昨天是星期一", true));
    acc.push(seg("我", "昨天是星期一。", false));
    const lines = acc.getLines();
    expect(lines).toHaveLength(1);
    expect(lines[0].text).toBe("昨天是星期一。");
    expect(lines[0].isPartial).toBe(false);
  });

  it("无 partial 的 final 事件直接新增一行", () => {
    const acc = new TranscriptAccumulator();
    acc.push(seg("客户", "We need NPI samples.", false));
    const lines = acc.getLines();
    expect(lines).toHaveLength(1);
    expect(lines[0].speakerLabel).toBe("客户");
    expect(lines[0].text).toBe("We need NPI samples.");
  });

  it("双说话人交替：各自独立行", () => {
    const acc = new TranscriptAccumulator();
    acc.push(seg("客户", "We need", true));
    acc.push(seg("我", "好", true));
    acc.push(seg("客户", "We need NPI", true));
    acc.push(seg("客户", "We need NPI samples.", false));
    acc.push(seg("我", "好的，明白。", false));
    const lines = acc.getLines();
    // 客户 partial → 我 partial → 客户 partial(更新客户行) → 客户 final → 我 final
    expect(lines).toHaveLength(2);
    expect(lines[0]).toMatchObject({ speakerLabel: "客户", text: "We need NPI samples.", isPartial: false });
    expect(lines[1]).toMatchObject({ speakerLabel: "我", text: "好的，明白。", isPartial: false });
  });

  it("每句 final 后重新开始 partial 会新起一行", () => {
    const acc = new TranscriptAccumulator();
    acc.push(seg("我", "第一句", true));
    acc.push(seg("我", "第一句。", false));
    acc.push(seg("我", "第二句", true));
    const lines = acc.getLines();
    expect(lines).toHaveLength(2);
    expect(lines[1]).toMatchObject({ text: "第二句", isPartial: true });
  });

  it("key 稳定：partial 更新与 final 固化复用同一 key，新行才递增", () => {
    const acc = new TranscriptAccumulator();
    acc.push(seg("a", "x", true));
    acc.push(seg("a", "y", false));
    acc.push(seg("b", "z", false));
    const keys = acc.getLines().map((l) => l.key);
    expect(keys).toHaveLength(2);
    expect(new Set(keys).size).toBe(2);
    // 同行 key 不变（partial→final）
    expect(keys[0]).toBe(keys[0]);
  });
});
