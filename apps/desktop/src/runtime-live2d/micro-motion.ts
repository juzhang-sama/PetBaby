export interface MicroMotionFrame {
  breath: number;
  bodySway: number;
}

const TAU = Math.PI * 2;
const MAX_DELTA_MS = 100;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function normalizeDelta(deltaMs: number): number {
  return Number.isFinite(deltaMs) ? clamp(deltaMs, 0, MAX_DELTA_MS) : 0;
}

export class MicroMotionController {
  private elapsedMs = 0;
  private paused = false;
  private carried = false;
  private frame: MicroMotionFrame = { breath: 0, bodySway: 0 };

  setCarried(carried: boolean): void {
    this.carried = carried;
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
  }

  update(deltaMs: number): MicroMotionFrame {
    if (this.paused) return this.frame;

    this.elapsedMs += normalizeDelta(deltaMs);
    const breathWave = Math.sin(TAU * this.elapsedMs / 4_000);
    const bodySway = this.carried
      ? 0
      : clamp(
          4.5 * Math.sin(TAU * this.elapsedMs / 7_500)
            + 1.5 * Math.sin(TAU * this.elapsedMs / 11_300),
          -6,
          6,
        );

    this.frame = {
      breath: this.carried ? breathWave * 0.25 : breathWave,
      bodySway,
    };
    return this.frame;
  }
}
