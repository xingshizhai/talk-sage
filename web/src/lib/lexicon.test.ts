import { describe, expect, it } from "vitest";
import { lexiconDraftToPack, lexiconIdForScene } from "./lexicon";

describe("lexicon", () => {
  it("只返回当前场景已绑定且仍然存在的词库", () => {
    expect(lexiconIdForScene({ meeting: "weekly", dictation: "gone" }, "meeting", { weekly: {} })).toBe("weekly");
    expect(lexiconIdForScene({ meeting: "weekly" }, "dictation", { weekly: {} })).toBeNull();
    expect(lexiconIdForScene({ dictation: "gone" }, "dictation", { weekly: {} })).toBeNull();
  });

  it("词库草稿能还原说明、术语、纠错和人名", () => {
    expect(lexiconDraftToPack({
      id: "weekly",
      label: "周例会",
      description: "项目例会",
      terms: "TalkSage\n",
      corrections: "拓思者 => TalkSage",
      people: [{ name: "张三", aliases: "张总" }],
    })).toEqual({
      label: "周例会",
      description: "项目例会",
      terms: ["TalkSage"],
      corrections: { 拓思者: "TalkSage" },
      people: [["张三", "张总"]],
    });
  });
});