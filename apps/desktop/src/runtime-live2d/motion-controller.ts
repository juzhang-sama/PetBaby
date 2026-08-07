import type { PetMotion, PetMotionHandle } from "../runtime/pet-renderer";

export interface MotionPlaybackOptions {
  priority: number;
  loop: boolean;
}

export interface MotionPlaybackPort {
  start(name: PetMotion, options: MotionPlaybackOptions, onFinished: () => void): PetMotionHandle;
  stopAll(): void;
}

export interface ActiveMotion extends MotionPlaybackOptions {
  name: PetMotion;
  startedAt: number;
}

export interface MotionControllerOptions {
  port: MotionPlaybackPort;
  resumeForState?: () => ({ name: PetMotion } & MotionPlaybackOptions) | null;
  now?: () => number;
}

const NOOP_HANDLE: PetMotionHandle = { cancel() {} };

export class MotionController {
  private active: (ActiveMotion & { token: symbol; handle: PetMotionHandle; cancelled: boolean }) | null = null;
  private stopped = false;
  private readonly now: () => number;

  constructor(private readonly options: MotionControllerOptions) {
    this.now = options.now ?? (() => performance.now());
  }

  play(name: PetMotion, options: Partial<MotionPlaybackOptions> = {}): PetMotionHandle {
    const requested = { priority: options.priority ?? 0, loop: options.loop ?? false };
    if (this.stopped || (this.active && requested.priority < this.active.priority)) return NOOP_HANDLE;

    this.cancelActive();
    const token = Symbol(name);
    const state = {
      name,
      ...requested,
      startedAt: this.now(),
      token,
      handle: NOOP_HANDLE,
      cancelled: false,
    };
    this.active = state;
    state.handle = this.options.port.start(name, requested, () => this.finish(token));

    let cancelled = false;
    return {
      cancel: () => {
        if (cancelled) return;
        cancelled = true;
        if (this.active?.token !== token) return;
        this.cancelActive();
      },
    };
  }

  current(): ActiveMotion | null {
    if (!this.active) return null;
    const { name, priority, loop, startedAt } = this.active;
    return { name, priority, loop, startedAt };
  }

  stopAll(): void {
    if (this.stopped) return;
    this.stopped = true;
    this.cancelActive();
    this.options.port.stopAll();
  }

  private finish(token: symbol): void {
    if (this.active?.token !== token) return;
    const wasLoop = this.active.loop;
    this.active = null;
    if (wasLoop || this.stopped) return;
    const resume = this.options.resumeForState?.();
    if (resume) this.play(resume.name, resume);
  }

  private cancelActive(): void {
    const active = this.active;
    this.active = null;
    if (!active || active.cancelled) return;
    active.cancelled = true;
    active.handle.cancel();
  }
}
