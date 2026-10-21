/** 重连后要写回计时器的状态。`start === 0` 表示暂停，时长冻在 `accumMs`。 */
export interface RestoredListenTimer {
  listening: boolean;
  paused: boolean;
  timer: { start: number; accumMs: number };
  elapsedSec: number;
}

/**
 * 用快照里的阶段和已累计时长恢复按钮与计时。
 *
 * 有 `elapsedMs` 时从那场监听续上，而不是从 0 开始。没有这个字段（旧服务端）时，
 * 录音态仍回到「正在监听」，但计时只能从当前时刻起算。
 */
export function timerFromSnapshot(
  stage: string,
  elapsedMs: number | undefined,
  now: number,
): RestoredListenTimer | null {
  const elapsed = typeof elapsedMs === "number" && Number.isFinite(elapsedMs) && elapsedMs > 0 ? elapsedMs : 0;
  if (stage === "recording") {
    return {
      listening: true,
      paused: false,
      timer: { start: now, accumMs: elapsed },
      elapsedSec: Math.floor(elapsed / 1000),
    };
  }
  if (stage === "paused") {
    return {
      listening: true,
      paused: true,
      timer: { start: 0, accumMs: elapsed },
      elapsedSec: Math.floor(elapsed / 1000),
    };
  }
  if (stage === "idle" || stage === "asr_ready") {
    return {
      listening: false,
      paused: false,
      timer: { start: 0, accumMs: 0 },
      elapsedSec: 0,
    };
  }
  return null;
}
