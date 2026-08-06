import { ClipPlayer, type AnimClip } from "./anim-clip";
import { easeOutCubic } from "./easing";
import { blendParams, defaultParams, mergeParams, type Params } from "./params";

export interface AnimStateDef {
  id: string;
  clip: AnimClip;
  followUp?: string;
  blendMs?: number;
}

const DEFAULT_BLEND_MS = 150;

export class AnimStateMachine {
  private readonly statesById = new Map<string, AnimStateDef>();
  private currentId: string;
  private current: ClipPlayer;
  private previous: { player: ClipPlayer; startedAt: number; blendMs: number } | null = null;
  private lastNow = 0;

  constructor(states: AnimStateDef[], initialId: string) {
    for (const state of states) {
      this.statesById.set(state.id, state);
    }
    const initial = this.require(initialId);
    this.currentId = initial.id;
    this.current = new ClipPlayer(initial.clip);
    this.current.start(0);
  }

  get stateId(): string {
    return this.currentId;
  }

  play(id: string, now: number, blendMs?: number): void {
    if (id === this.currentId) return;
    const def = this.require(id);
    this.previous = {
      player: this.current,
      startedAt: now,
      blendMs: blendMs ?? def.blendMs ?? DEFAULT_BLEND_MS,
    };
    this.currentId = def.id;
    this.current = new ClipPlayer(def.clip);
    this.current.start(now);
    this.lastNow = now;
  }

  update(now: number): void {
    this.lastNow = now;
    this.current.sample(now);
    if (this.current.finished) {
      const def = this.require(this.currentId);
      if (def.followUp && !def.clip.loop) {
        this.play(def.followUp, now);
      }
    }
  }

  params(): Params {
    const current = mergeParams(defaultParams(), this.current.sample(this.lastNow));
    if (!this.previous) return current;
    const elapsed = this.lastNow - this.previous.startedAt;
    const weight = this.previous.blendMs <= 0
      ? 1
      : easeOutCubic(Math.min(1, elapsed / this.previous.blendMs));
    const previous = mergeParams(defaultParams(), this.previous.player.sample(this.lastNow));
    return blendParams(previous, current, weight);
  }

  private require(id: string): AnimStateDef {
    const def = this.statesById.get(id);
    if (!def) throw new Error(`unknown animation state: ${id}`);
    return def;
  }
}
