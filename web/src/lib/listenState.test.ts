import { describe, expect, it } from "vitest";
import { timerFromSnapshot } from "./listenState";

describe("timerFromSnapshot", () => {
  it("continues an active listen from the elapsed time instead of zero", () => {
    const restored = timerFromSnapshot("recording", 90_000, 1_700_000_000_000);
    expect(restored).toEqual({
      listening: true,
      paused: false,
      timer: { start: 1_700_000_000_000, accumMs: 90_000 },
      elapsedSec: 90,
    });
  });

  it("freezes a paused listen at the elapsed time", () => {
    const restored = timerFromSnapshot("paused", 12_500, 50);
    expect(restored).toMatchObject({
      listening: true,
      paused: true,
      timer: { start: 0, accumMs: 12_500 },
      elapsedSec: 12,
    });
  });

  it("shows the start button only when the backend session is idle", () => {
    expect(timerFromSnapshot("idle", 40_000, 10)).toMatchObject({ listening: false, elapsedSec: 0 });
    expect(timerFromSnapshot("asr_ready", 0, 10)).toMatchObject({ listening: false });
  });

  it("keeps listening when an older snapshot has no elapsed time", () => {
    expect(timerFromSnapshot("recording", undefined, 80)).toMatchObject({
      listening: true,
      timer: { start: 80, accumMs: 0 },
      elapsedSec: 0,
    });
  });
});
