import type { NudgeEvent } from "./api";

/** 会中提示同时只展示一条。已在展示的同类同文案不叠第二张卡片。 */
export function applyIncomingNudge(prev: NudgeEvent[], incoming: NudgeEvent): NudgeEvent[] {
  const current = prev[0];
  if (current && current.kind === incoming.kind && current.message === incoming.message) {
    return prev;
  }
  return [incoming];
}
