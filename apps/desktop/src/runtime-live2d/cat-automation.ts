import type { CatAutomationFrame } from "./cat-motion-contract";

export type CatAutomationMode = "idle" | "pointerFocus" | "dragging" | "paused";

export interface CatAutomationControllerOptions {
  random?: () => number;
}

const BLINK_INTERVAL_MS = 4_200;
export const CAT_BLINK_DURATION_MS = 220;
export const CAT_FIRST_BLINK_DELAY_MS = 2_800;

const OPEN_FRAME: CatAutomationFrame = {
  breath: 0.5,
  eyeLeftOpen: 1,
  eyeRightOpen: 1,
  earLeft: 0,
  earRight: 0,
  tailAngle: 0,
  tailCurl: 0,
  tailTip: 0,
};

export function sampleBlinkEye(progress: number, phaseOffset: number): number {
  const clampedProgress = Math.min(1, Math.max(0, progress));
  const closedAt = Math.min(0.9, Math.max(0.1, 0.5 + phaseOffset));
  const edgeDistance = clampedProgress < closedAt ? closedAt : 1 - closedAt;
  const distanceFromClosed = Math.min(1, Math.abs(clampedProgress - closedAt) / edgeDistance);
  return smoothstep(distanceFromClosed);
}

export class CatAutomationController {
  private elapsedMs = 0;
  private readonly phase: number;
  private frame: CatAutomationFrame = { ...OPEN_FRAME };

  constructor(options: CatAutomationControllerOptions = {}) {
    const random = options.random ?? Math.random;
    this.phase = Math.min(1, Math.max(0, random())) * Math.PI * 2;
  }

  update(deltaMs: number, mode: CatAutomationMode): CatAutomationFrame {
    if (mode === "paused") return { ...this.frame };
    this.elapsedMs += Math.max(0, deltaMs);
    const seconds = this.elapsedMs / 1_000;
    const amplitude = mode === "pointerFocus" ? 1.35 : mode === "dragging" ? 0.15 : 1;
    const blinkElapsed = this.elapsedMs - CAT_FIRST_BLINK_DELAY_MS;
    const blinkTime = blinkElapsed < 0 ? CAT_BLINK_DURATION_MS : blinkElapsed % BLINK_INTERVAL_MS;
    const blinkProgress = blinkTime < CAT_BLINK_DURATION_MS ? blinkTime / CAT_BLINK_DURATION_MS : null;

    this.frame = {
      breath: clamp(0.5 + Math.sin(seconds * 1.8 + this.phase) * 0.16, 0, 1),
      eyeLeftOpen: blinkProgress === null ? 1 : sampleBlinkEye(blinkProgress, -0.035),
      eyeRightOpen: blinkProgress === null ? 1 : sampleBlinkEye(blinkProgress, 0.035),
      earLeft: clamp(Math.sin(seconds * 0.55 + this.phase) * 0.24 * amplitude, -0.35, 0.35),
      earRight: clamp(Math.sin(seconds * 0.47 + this.phase + 1.2) * 0.22 * amplitude, -0.35, 0.35),
      tailAngle: clamp(Math.sin(seconds * 0.7 + this.phase) * 8 * amplitude, -12, 12),
      tailCurl: clamp(Math.sin(seconds * 0.41 + this.phase + 0.7) * 0.3 * amplitude, -0.45, 0.45),
      tailTip: clamp(Math.sin(seconds * 1.05 + this.phase + 1.5) * 0.42 * amplitude, -0.65, 0.65),
    };
    return { ...this.frame };
  }
}

function smoothstep(value: number): number {
  const clamped = clamp(value, 0, 1);
  return clamped * clamped * (3 - 2 * clamped);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
