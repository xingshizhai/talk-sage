import { describe, expect, it } from "vitest";
import type { NudgeEvent } from "./api";
import { applyIncomingNudge } from "./nudge";

function nudge(partial: Partial<NudgeEvent> & Pick<NudgeEvent, "id" | "message">): NudgeEvent {
  return {
    kind: "talk_ratio",
    severity: "medium",
    action: "ask_question",
    timestamp_ms: 1,
    ...partial,
  };
}

describe("applyIncomingNudge", () => {
  const first = nudge({
    id: "1",
    message: "你主导了大部分对话——多倾听客户，留出发言空间。",
  });
  const duplicate = nudge({
    id: "2",
    message: "你主导了大部分对话——多倾听客户，留出发言空间。",
    timestamp_ms: 2,
  });
  const otherKind = nudge({
    id: "3",
    kind: "pace",
    severity: "low",
    message: "语速偏快——放慢节奏，客户更容易跟上。",
    action: null,
  });

  it("shows the first tip as the only card", () => {
    expect(applyIncomingNudge([], first)).toEqual([first]);
  });

  it("does not stack a second copy of the same tip", () => {
    expect(applyIncomingNudge([first], duplicate)).toEqual([first]);
  });

  it("replaces the card when a different tip arrives", () => {
    expect(applyIncomingNudge([first], otherKind)).toEqual([otherKind]);
  });
});
