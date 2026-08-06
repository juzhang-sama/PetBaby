import { easeByName, type EasingName } from "./easing";
import type { ParamKey, Params } from "./params";

export interface Keyframe {
  timeMs: number;
  value: number;
  ease?: EasingName;
}

export interface AnimClip {
  name: string;
  durationMs: number;
  loop?: boolean;
  tracks: Partial<Record<ParamKey, Keyframe[]>>;
}

function normalizeTime(timeMs: number, durationMs: number, loop: boolean): number {
  if (!loop || durationMs <= 0) return timeMs;
  return timeMs % durationMs;
}

export function sampleTrack(
  keyframes: Keyframe[],
  timeMs: number,
  loop: boolean,
  durationMs?: number,
): number {
  if (keyframes.length === 0) return 0;
  const local = normalizeTime(timeMs, durationMs ?? 0, loop);
  if (local <= keyframes[0]!.timeMs) return keyframes[0]!.value;
  const last = keyframes[keyframes.length - 1]!;
  if (local >= last.timeMs) return last.value;
  for (let i = 0; i < keyframes.length - 1; i += 1) {
    const from = keyframes[i]!;
    const to = keyframes[i + 1]!;
    if (local >= from.timeMs && local <= to.timeMs) {
      const span = to.timeMs - from.timeMs;
      const raw = span <= 0 ? 1 : (local - from.timeMs) / span;
      const eased = easeByName(to.ease ?? "linear")(raw);
      return from.value + (to.value - from.value) * eased;
    }
  }
  return last.value;
}

export function sampleClip(clip: AnimClip, timeMs: number): Partial<Params> {
  const out: Partial<Params> = {};
  for (const [key, keyframes] of Object.entries(clip.tracks)) {
    if (!keyframes || keyframes.length === 0) continue;
    out[key as ParamKey] = sampleTrack(
      keyframes,
      timeMs,
      clip.loop ?? false,
      clip.durationMs,
    );
  }
  return out;
}

export class ClipPlayer {
  private startTime = 0;
  private started = false;
  private lastTime = 0;

  constructor(private readonly clip: AnimClip) {}

  start(now: number): void {
    this.startTime = now;
    this.started = true;
  }

  get finished(): boolean {
    if (!this.started) return false;
    if (this.clip.loop) return false;
    return this.lastTime >= this.clip.durationMs;
  }

  sample(now: number): Partial<Params> {
    if (!this.started) return {};
    this.lastTime = this.currentTime(now);
    return sampleClip(this.clip, this.lastTime);
  }

  private currentTime(now?: number): number {
    const t = now ?? this.startTime;
    return Math.max(0, t - this.startTime);
  }
}
