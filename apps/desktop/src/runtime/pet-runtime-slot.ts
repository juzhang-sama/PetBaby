import type { PetRendererRuntime } from "./pet-renderer-bootstrap";
import type { PetCalibrationV1 } from "./pet-calibration";
import type {
  PetExpression,
  PetHitArea,
  PetMotion,
  PetMotionHandle,
  PetRenderAsset,
  PetRenderer,
} from "./pet-renderer";
import type { CatMotionNameV1 } from "../runtime-live2d/cat-motion-contract";
import type { CatAutomationMode } from "../runtime-live2d/cat-automation";

export interface MountedPetRuntime extends PetRendererRuntime {
  petId: string;
  isPreviewFallback?(): boolean;
}

export interface PreparedRuntimeSwap {
  previous: MountedPetRuntime;
  candidate: MountedPetRuntime;
  activate(): void;
  commit(): void;
  rollback(): void;
}

interface Viewport {
  width: number;
  height: number;
  dpr: number;
}

interface PendingRuntimeSwap {
  previous: MountedPetRuntime;
  candidate: MountedPetRuntime;
  activated: boolean;
  activationFailed: boolean;
  settled: boolean;
  surfaceRestored: boolean;
  viewportRestored: boolean;
  visibilityRestored: boolean;
  candidateDestroyed: boolean;
  candidateIdle: ContinuousIdleMotion | null;
}

interface ContinuousIdleMotion {
  options: { loop: true; priority?: number };
  owners: Map<MountedPetRuntime, PetMotionHandle>;
  cancelled: boolean;
}

export class PetRuntimeSlot implements PetRenderer {
  private active: MountedPetRuntime;
  private viewport: Viewport | null = null;
  private visible = false;
  private pending: PendingRuntimeSwap | null = null;
  private continuousIdle: ContinuousIdleMotion | null = null;
  private destroyed = false;

  constructor(private readonly root: HTMLElement, initial: MountedPetRuntime) {
    this.active = initial;
    this.root.replaceChildren(initial.getSurface());
  }

  get activePetId(): string {
    return this.active.petId;
  }

  getSurface(): HTMLCanvasElement {
    this.assertAlive();
    return this.active.getSurface();
  }

  getHitSurface(): HTMLCanvasElement {
    this.assertAlive();
    return this.active.getHitSurface();
  }

  refreshActiveSurface(runtime: MountedPetRuntime, afterAttach?: () => void): boolean {
    this.assertAlive();
    if (this.active !== runtime) return false;
    this.root.replaceChildren(runtime.getSurface());
    afterAttach?.();
    return true;
  }

  prepare(candidate: MountedPetRuntime): PreparedRuntimeSwap {
    this.assertAlive();
    if (candidate === this.active) throw new Error("candidate is already active");
    if (this.pending) {
      if (candidate !== this.pending.previous && candidate !== this.pending.candidate) {
        this.destroyQuietly(candidate);
      }
      throw new Error("swap is already pending");
    }
    try {
      if (this.viewport) candidate.host.resize(this.viewport);
      candidate.host.setVisibility(this.visible);
      candidate.host.update(0);
    } catch (error) {
      try {
        candidate.host.destroy();
      } catch {
        // Preserve the original preparation failure.
      }
      throw error;
    }

    const state: PendingRuntimeSwap = {
      previous: this.active,
      candidate,
      activated: false,
      activationFailed: false,
      settled: false,
      surfaceRestored: false,
      viewportRestored: false,
      visibilityRestored: false,
      candidateDestroyed: false,
      candidateIdle: null,
    };
    this.pending = state;

    return {
      previous: state.previous,
      candidate,
      activate: () => {
        if (!this.isPending(state) || state.activated || state.activationFailed || this.active !== state.previous) {
          throw new Error("swap is not activatable");
        }
        try {
          state.previous.host.setVisibility(false);
          this.root.replaceChildren(candidate.getSurface());
          this.active = candidate;
          const continuousIdle = this.continuousIdle;
          if (continuousIdle && !continuousIdle.cancelled) {
            const handle = candidate.host.playMotion("idle", continuousIdle.options);
            continuousIdle.owners.set(candidate, handle);
            state.candidateIdle = continuousIdle;
          }
          state.activated = true;
        } catch (error) {
          this.abortActivation(state);
          throw error;
        }
      },
      commit: () => {
        if (state.settled || !this.isPending(state)) return;
        if (!state.activated || this.active !== candidate) throw new Error("swap is not committable");
        state.settled = true;
        this.pending = null;
        try {
          state.previous.host.destroy();
        } finally {
          this.removeIdleOwner(state.previous, state);
        }
      },
      rollback: () => {
        if (state.settled || !this.isPending(state)) return;
        this.rollbackState(state);
      },
    };
  }

  load(asset: PetRenderAsset): Promise<void> {
    this.assertAlive();
    return this.active.host.load(asset);
  }

  resize(viewport: Viewport): void {
    this.assertAlive();
    this.viewport = { ...viewport };
    this.active.host.resize(viewport);
  }

  playMotion(motion: PetMotion, options?: { loop?: boolean; priority?: number }): PetMotionHandle {
    this.assertAlive();
    const handle = this.active.host.playMotion(motion, options);
    if (motion !== "idle" || options?.loop !== true) return handle;

    const continuousIdle: ContinuousIdleMotion = {
      options: { ...options, loop: true },
      owners: new Map([[this.active, handle]]),
      cancelled: false,
    };
    this.continuousIdle = continuousIdle;
    return {
      cancel: () => {
        if (continuousIdle.cancelled) return;
        continuousIdle.cancelled = true;
        let firstError: unknown;
        let failed = false;
        for (const owner of new Set(continuousIdle.owners.values())) {
          try {
            owner.cancel();
          } catch (error) {
            if (!failed) firstError = error;
            failed = true;
          }
        }
        continuousIdle.owners.clear();
        if (this.continuousIdle === continuousIdle) this.continuousIdle = null;
        if (failed) throw firstError;
      },
    };
  }

  supportsCatMotionV1(): boolean {
    this.assertAlive();
    return this.active.host.supportsCatMotionV1();
  }

  setCatAutomationMode(mode: CatAutomationMode): void {
    this.assertAlive();
    this.active.host.setCatAutomationMode(mode);
  }

  playCatMotion(
    motion: CatMotionNameV1,
    transition: { loop?: boolean; priority?: number; fadeInMs?: number; fadeOutMs?: number } = {},
    onFinished?: () => void,
  ): PetMotionHandle {
    this.assertAlive();
    return this.active.host.playCatMotion(motion, transition, onFinished);
  }

  setExpression(value: PetExpression, weight?: number): void {
    this.assertAlive();
    this.active.host.setExpression(value, weight);
  }

  setLookTarget(value: { x: number; y: number } | null): void {
    this.assertAlive();
    this.active.host.setLookTarget(value);
  }

  setLipSync(value: number): void {
    this.assertAlive();
    this.active.host.setLipSync(value);
  }

  setCalibration(value: PetCalibrationV1): void {
    this.assertAlive();
    this.active.host.setCalibration(value);
  }

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    this.assertAlive();
    return this.active.host.hitTest(point);
  }

  setVisibility(visible: boolean): void {
    this.assertAlive();
    this.visible = visible;
    this.active.host.setVisibility(visible);
  }

  update(deltaMs: number): void {
    this.assertAlive();
    this.active.host.update(deltaMs);
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    const pending = this.pending;
    this.pending = null;
    if (pending) {
      pending.settled = true;
      const inactive = this.active === pending.previous ? pending.candidate : pending.previous;
      this.destroySlotRuntimes([this.active, inactive], pending);
      return;
    }
    this.destroySlotRuntimes([this.active]);
  }

  private isPending(state: PendingRuntimeSwap): boolean {
    return !this.destroyed && this.pending === state;
  }

  private assertAlive(): void {
    if (this.destroyed) throw new Error("PetRuntimeSlot has been destroyed");
  }

  private abortActivation(state: PendingRuntimeSwap): void {
    state.activationFailed = true;
    this.active = state.previous;
    try {
      this.rollbackState(state);
    } catch {
      // Preserve the activation failure; rollback can resume incomplete compensation.
    }
  }

  private rollbackState(state: PendingRuntimeSwap): void {
    const expectedActive = state.activated && !state.surfaceRestored
      ? state.candidate
      : state.previous;
    if (this.active !== expectedActive) throw new Error("swap is not rollbackable");
    if (state.activated || state.activationFailed) {
      if (!state.surfaceRestored) {
        this.root.replaceChildren(state.previous.getSurface());
        this.active = state.previous;
        state.surfaceRestored = true;
      }
      if (!state.viewportRestored) {
        if (this.viewport) state.previous.host.resize(this.viewport);
        state.viewportRestored = true;
      }
      if (!state.visibilityRestored) {
        state.previous.host.setVisibility(this.visible);
        state.visibilityRestored = true;
      }
    }
    if (!state.candidateDestroyed) {
      state.candidate.host.destroy();
      this.removeIdleOwner(state.candidate, state);
      state.candidateDestroyed = true;
    }
    state.settled = true;
    this.pending = null;
  }

  private destroyQuietly(runtime: MountedPetRuntime): void {
    try {
      runtime.host.destroy();
    } catch {
      // The caller is handling a more relevant failure.
    }
  }

  private removeIdleOwner(runtime: MountedPetRuntime, pending?: PendingRuntimeSwap): void {
    this.continuousIdle?.owners.delete(runtime);
    pending?.candidateIdle?.owners.delete(runtime);
  }

  private destroySlotRuntimes(
    runtimes: MountedPetRuntime[],
    pending?: PendingRuntimeSwap,
  ): void {
    let firstError: unknown;
    let failed = false;
    for (const runtime of new Set(runtimes)) {
      try {
        runtime.host.destroy();
      } catch (error) {
        if (!failed) firstError = error;
        failed = true;
      } finally {
        this.removeIdleOwner(runtime, pending);
      }
    }
    this.continuousIdle = null;
    if (failed) throw firstError;
  }
}
