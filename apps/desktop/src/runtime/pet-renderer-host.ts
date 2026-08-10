import type {
  PetExpression,
  PetHitArea,
  PetMotion,
  PetMotionHandle,
  PetRenderAsset,
  PetRenderer,
} from "./pet-renderer";

interface Viewport {
  width: number;
  height: number;
  dpr: number;
}

export class PetRendererHost implements PetRenderer {
  private current: PetRenderer;
  private viewport: Viewport | null = null;
  private visible = false;
  private destroyed = false;
  private replacementGeneration = 0;

  constructor(renderer: PetRenderer) {
    this.current = renderer;
  }

  load(asset: PetRenderAsset): Promise<void> {
    this.assertAlive();
    return this.current.load(asset);
  }

  resize(viewport: Viewport): void {
    this.assertAlive();
    this.viewport = { ...viewport };
    this.current.resize(viewport);
  }

  playMotion(motion: PetMotion, options?: { loop?: boolean; priority?: number }): PetMotionHandle {
    return this.current.playMotion(motion, options);
  }

  setExpression(expression: PetExpression, weight?: number): void {
    this.current.setExpression(expression, weight);
  }

  setLookTarget(target: { x: number; y: number } | null): void {
    this.current.setLookTarget(target);
  }

  setLipSync(value: number): void {
    this.current.setLipSync(value);
  }

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    return this.current.hitTest(point);
  }

  setVisibility(visible: boolean): void {
    if (this.destroyed) return;
    this.visible = visible;
    this.current.setVisibility(visible);
  }

  update(deltaMs: number): void {
    if (this.destroyed) return;
    this.current.update(deltaMs);
  }

  async replace(renderer: PetRenderer, asset: PetRenderAsset): Promise<void> {
    this.assertAlive();
    const generation = ++this.replacementGeneration;
    try {
      await renderer.load(asset);
      if (this.destroyed || generation !== this.replacementGeneration) {
        renderer.destroy();
        return;
      }
      if (this.viewport) renderer.resize(this.viewport);
      renderer.setVisibility(this.visible);
    } catch (error) {
      renderer.destroy();
      throw error;
    }

    const previous = this.current;
    this.current = renderer;
    previous.destroy();
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.replacementGeneration += 1;
    this.current.destroy();
  }

  private assertAlive(): void {
    if (this.destroyed) throw new Error("PetRendererHost has been destroyed");
  }
}
