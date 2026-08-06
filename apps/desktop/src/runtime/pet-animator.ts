import type { BehaviorIntent } from "../behavior/intents";
import { AnimStateMachine, type AnimStateDef } from "./anim-machine";
import { createPetClips, type PetClips } from "./pet-clips";

export interface AnimatorDriver {
  setEyesOpen(open: boolean): void;
  setBreathPhase(phase: number): void;
  scaleSquash(factor: number): void;
  shift(dx: number, dy: number): void;
  setAccentVisible(visible: boolean): void;
  setTilt(angleDeg: number): void;
  setHeadTurn?(amount: number): void;
}

export type AnimatorMode = "idle" | "interact" | "carried" | "falling" | "strolling";

const BREATH_PERIOD_MS = 4_000;
const BLINK_CLOSED_MS = 160;
const LOOK_TILT_DEG = 6;

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

/**
 * Timeline-driven pet animator. Continuous motion is expressed as keyframe
 * clips played by an AnimStateMachine; discrete events (blink, sleep) are
 * layered on top. Every channel finally flows through AnimatorDriver.
 */
export class PetAnimator {
  private running = false;
  private lastTick = 0;
  private mode: AnimatorMode = "idle";
  private sleeping = false;
  private readonly clips: PetClips;
  private readonly machine: AnimStateMachine;
  private readonly blinkScheduler = new BlinkScheduler(3_000, 8_000);
  private nextBlinkAt = 0;
  private blinkingUntil = 0;

  constructor(private readonly driver: AnimatorDriver) {
    this.clips = createPetClips();
    const states: AnimStateDef[] = [
      { id: "idle", clip: this.clips.idle },
      { id: "sleep", clip: this.clips.sleep },
      { id: "look-left", clip: this.clips["look-left"], followUp: "idle" },
      { id: "look-right", clip: this.clips["look-right"], followUp: "idle" },
      { id: "react-happy", clip: this.clips["react-happy"], followUp: "idle" },
      { id: "react-curious", clip: this.clips["react-curious"], followUp: "idle" },
      { id: "carried", clip: this.clips.carried },
      { id: "landed", clip: this.clips.landed, followUp: "idle" },
      { id: "falling", clip: this.clips.falling },
      { id: "stroll", clip: this.clips.stroll },
    ];
    this.machine = new AnimStateMachine(states, "idle");
  }

  start(): void {
    this.running = true;
  }

  stop(): void {
    this.running = false;
  }

  setMode(mode: AnimatorMode): void {
    this.mode = mode;
    if (mode === "carried" || mode === "falling") {
      this.driver.setEyesOpen(true);
      this.machine.play(mode, this.lastTick);
    } else if (mode === "strolling") {
      this.machine.play("stroll", this.lastTick);
    } else if (mode === "idle") {
      this.machine.play("idle", this.lastTick);
    }
  }

  setIntent(intent: BehaviorIntent): void {
    switch (intent.type) {
      case "react-happy":
        this.machine.play("react-happy", this.lastTick);
        break;
      case "react-curious":
        this.machine.play("react-curious", this.lastTick);
        break;
      case "look":
        this.machine.play(
          intent.target === "left" ? "look-left"
            : intent.target === "right" ? "look-right"
              : "idle",
          this.lastTick,
        );
        break;
      case "carried":
        this.setMode("carried");
        break;
      case "falling":
        this.setMode("falling");
        break;
      case "stroll":
        this.setMode("strolling");
        break;
      case "landed":
        this.mode = "idle";
        this.machine.play("landed", this.lastTick);
        break;
      case "sleep":
        this.sleeping = true;
        this.mode = "idle";
        this.machine.play("sleep", this.lastTick);
        break;
      case "awake":
        this.sleeping = false;
        this.machine.play("idle", this.lastTick);
        break;
      case "blink":
        this.forceBlink();
        break;
    }
  }

  forceBlink(): void {
    this.blinkingUntil = this.lastTick + BLINK_CLOSED_MS;
  }

  tick(now: number): void {
    if (!this.running) return;
    this.lastTick = now;
    this.machine.update(now);
    const params = this.machine.params();

    // breathing stops while carried or falling
    if (this.mode !== "carried" && this.mode !== "falling") {
      this.driver.setBreathPhase(params.breath);
    }

    // body tilt and head turn follow the look channel; carried/falling use
    // their own wobble clips instead
    const lookX = params.lookX;
    if (this.mode === "carried" || this.mode === "falling") {
      this.driver.setTilt(params.tilt);
    } else {
      this.driver.setTilt(lookX * LOOK_TILT_DEG);
    }
    this.driver.setHeadTurn?.(lookX);

    if (params.squash !== 1) {
      this.driver.scaleSquash(params.squash);
    }
    this.driver.shift(params.shiftX, params.shiftY);
    this.driver.setAccentVisible(params.accent > 0.5);

    if (this.sleeping) {
      this.driver.setEyesOpen(false);
      return;
    }
    if (this.mode === "carried" || this.mode === "falling") {
      this.driver.setEyesOpen(true);
      return;
    }

    // blink scheduling: capture due-ness BEFORE nextAt() re-schedules
    const blinkDue = this.nextBlinkAt !== 0 && now >= this.nextBlinkAt;
    this.nextBlinkAt = this.blinkScheduler.nextAt(now);
    const eyesOpen = now > this.blinkingUntil;
    this.driver.setEyesOpen(eyesOpen);
    if (blinkDue && now > this.blinkingUntil) {
      this.forceBlink();
    }
  }
}
