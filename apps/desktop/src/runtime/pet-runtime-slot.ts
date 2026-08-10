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

export class PetRuntimeSlot implements PetRenderer {
  private active: MountedPetRuntime;
  private viewport: Viewport | null = null;
  private visible = false;

  constructor(private readonly root: HTMLElement, initial: MountedPetRuntime) {
    this.active = initial;
    this.root.replaceChildren(initial.getSurface());
  }

  get activePetId(): string {
    return this.active.petId;
  }

  getSurface(): HTMLCanvasElement {
    return this.active.getSurface();
  }

  prepare(candidate: MountedPetRuntime): PreparedRuntimeSwap {
    if (this.viewport) candidate.host.resize(this.viewport);
    candidate.host.setVisibility(this.visible);
    candidate.host.update(0);

    const previous = this.active;
    let activated = false;
    let settled = false;

    return {
      previous,
      candidate,
      activate: () => {
        if (settled || activated) throw new Error("swap is not activatable");
        previous.host.setVisibility(false);
        this.active = candidate;
        this.root.replaceChildren(candidate.getSurface());
        activated = true;
      },
      commit: () => {
        if (!activated || settled) throw new Error("swap is not committable");
        settled = true;
        previous.host.destroy();
      },
      rollback: () => {
        if (settled) return;
        settled = true;
        if (activated) {
          this.active = previous;
          this.root.replaceChildren(previous.getSurface());
          previous.host.setVisibility(this.visible);
        }
        candidate.host.destroy();
      },
    };
  }

  load(asset: PetRenderAsset): Promise<void> {
    return this.active.host.load(asset);
  }

  resize(viewport: Viewport): void {
    this.viewport = { ...viewport };
    this.active.host.resize(viewport);
  }

  playMotion(motion: PetMotion, options?: { loop?: boolean; priority?: number }): PetMotionHandle {
    return this.active.host.playMotion(motion, options);
  }

  setExpression(value: PetExpression, weight?: number): void {
    this.active.host.setExpression(value, weight);
  }

  setLookTarget(value: { x: number; y: number } | null): void {
    this.active.host.setLookTarget(value);
  }

  setLipSync(value: number): void {
    this.active.host.setLipSync(value);
  }

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    return this.active.host.hitTest(point);
  }

  setVisibility(visible: boolean): void {
    this.visible = visible;
    this.active.host.setVisibility(visible);
  }

  update(deltaMs: number): void {
    this.active.host.update(deltaMs);
  }

  destroy(): void {
    this.active.host.destroy();
  }
}
