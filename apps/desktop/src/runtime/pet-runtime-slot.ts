import type { PetRendererRuntime } from "./pet-renderer-bootstrap";
import type {
  PetExpression,
  PetHitArea,
  PetMotion,
  PetMotionHandle,
  PetRenderAsset,
  PetRenderer,
} from "./pet-renderer";

export interface MountedPetRuntime extends PetRendererRuntime {
  petId: string;
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
  settled: boolean;
}

export class PetRuntimeSlot implements PetRenderer {
  private active: MountedPetRuntime;
  private viewport: Viewport | null = null;
  private visible = false;
  private pending: PendingRuntimeSwap | null = null;
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

  prepare(candidate: MountedPetRuntime): PreparedRuntimeSwap {
    this.assertAlive();
    if (candidate === this.active) throw new Error("candidate is already active");
    if (this.pending) {
      candidate.host.destroy();
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
      settled: false,
    };
    this.pending = state;

    return {
      previous: state.previous,
      candidate,
      activate: () => {
        if (!this.isPending(state) || state.activated || this.active !== state.previous) {
          throw new Error("swap is not activatable");
        }
        state.previous.host.setVisibility(false);
        this.active = candidate;
        this.root.replaceChildren(candidate.getSurface());
        state.activated = true;
      },
      commit: () => {
        if (state.settled || !this.isPending(state)) return;
        if (!state.activated || this.active !== candidate) throw new Error("swap is not committable");
        state.settled = true;
        this.pending = null;
        state.previous.host.destroy();
      },
      rollback: () => {
        if (state.settled || !this.isPending(state)) return;
        const expectedActive = state.activated ? candidate : state.previous;
        if (this.active !== expectedActive) throw new Error("swap is not rollbackable");
        state.settled = true;
        this.pending = null;
        if (state.activated) {
          this.active = state.previous;
          this.root.replaceChildren(state.previous.getSurface());
          state.previous.host.setVisibility(this.visible);
        }
        candidate.host.destroy();
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
    return this.active.host.playMotion(motion, options);
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
      this.active.host.destroy();
      const inactive = this.active === pending.previous ? pending.candidate : pending.previous;
      inactive.host.destroy();
      return;
    }
    this.active.host.destroy();
  }

  private isPending(state: PendingRuntimeSwap): boolean {
    return !this.destroyed && this.pending === state;
  }

  private assertAlive(): void {
    if (this.destroyed) throw new Error("PetRuntimeSlot has been destroyed");
  }
}
