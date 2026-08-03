import type { BehaviorIntent } from "../behavior/intents";

export interface AnimatorDriver {
  setEyesOpen(open: boolean): void;
  setBreathPhase(phase: number): void;
  scaleSquash(factor: number): void;
  shift(dx: number, dy: number): void;
  setAccentVisible(visible: boolean): void;
}

export type AnimatorMode = "idle" | "interact" | "carried";

const BREATH_PERIOD_MS = 4_000;
const BLINK_CLOSED_MS = 160;
const BOUNCE_MS = 500;

export function breathPhaseAt(t: number): number {
  return ((t % BREATH_PERIOD_MS) / BREATH_PERIOD_MS) % 1;
}

export class BlinkScheduler {
  constructor(
    private readonly minMs: number,
    private readonly maxMs: number,
    private nextAtMs = 0,
  ) {}

  nextAt(now: number): number {
    if (this.nextAtMs > now) return this.nextAtMs;
    const delta = this.minMs + Math.random() * (this.maxMs - this.minMs);
    this.nextAtMs = now + delta;
    return this.nextAtMs;
  }
}

export class PetAnimator {
  private running = false;
  private lastTick = 0;
  private mode: AnimatorMode = "idle";
  private blinkScheduler = new BlinkScheduler(3_000, 8_000);
  private nextBlinkAt = 0;
  private blinkingUntil = 0;
  private bounceUntil = 0;

  constructor(private readonly driver: AnimatorDriver) {}

  start(): void {
    this.running = true;
  }

  stop(): void {
    this.running = false;
  }

  setMode(mode: AnimatorMode): void {
    this.mode = mode;
    if (mode === "carried") {
      this.driver.setEyesOpen(true);
    }
  }

  setIntent(intent: BehaviorIntent): void {
    if (intent.type === "react-happy") {
      this.bounceUntil = this.lastTick + BOUNCE_MS;
      this.driver.setAccentVisible(true);
    } else if (intent.type === "react-curious") {
      this.driver.shift(0, -6);
      this.driver.setAccentVisible(true);
    } else if (intent.type === "carried") {
      this.setMode("carried");
    } else if (intent.type === "landed") {
      this.setMode("idle");
      this.bounceUntil = this.lastTick + BOUNCE_MS;
    } else if (intent.type === "sleep") {
      this.setMode("idle");
      this.driver.setEyesOpen(false);
    } else if (intent.type === "awake") {
      this.driver.setEyesOpen(true);
    }
  }

  forceBlink(): void {
    this.blinkingUntil = this.lastTick + BLINK_CLOSED_MS;
  }

  tick(now: number): void {
    if (!this.running) return;
    const elapsed = this.lastTick === 0 ? 0 : now - this.lastTick;
    this.lastTick = now;

    if (this.mode !== "carried") {
      const phase = breathPhaseAt(now);
      this.driver.setBreathPhase(phase);
    }

    // blink scheduling
    if (this.mode !== "carried") {
      this.nextBlinkAt = this.blinkScheduler.nextAt(now);
      const eyesOpen = now > this.blinkingUntil;
      this.driver.setEyesOpen(eyesOpen);
      if (now >= this.nextBlinkAt && now > this.blinkingUntil) {
        this.forceBlink();
      }
    } else {
      this.driver.setEyesOpen(true);
    }

    // bounce feedback
    if (now < this.bounceUntil && this.mode !== "carried") {
      const progress = 1 - (this.bounceUntil - now) / BOUNCE_MS;
      const squash = 1 + Math.sin(progress * Math.PI) * 0.06;
      this.driver.scaleSquash(squash);
      this.driver.shift(0, -Math.sin(progress * Math.PI) * 10);
    }
  }
}
