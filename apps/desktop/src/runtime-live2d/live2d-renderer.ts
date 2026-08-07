import {
  isLive2DRenderAsset,
  type PetExpression,
  type PetHitArea,
  type PetMotion,
  type PetMotionHandle,
  type PetRenderAsset,
  type PetRenderer,
} from "../runtime/pet-renderer";
import {
  CubismModelLoader,
  type CubismModelLoaderPort,
  type LoadedCubismModel,
} from "./cubism-model-loader";
import { HitAreaResolver } from "./hit-area-resolver";
import { MotionController } from "./motion-controller";
import { ParameterMixer } from "./parameter-mixer";

type Live2DAsset = Extract<PetRenderAsset, { kind: "live2d" }>;
export type Live2DRendererStatus = "unloaded" | "loading" | "ready" | "context-lost" | "destroyed";

export interface Live2DRendererState {
  status: Live2DRendererStatus;
  visible: boolean;
}

export interface Live2DRendererOptions {
  loader?: CubismModelLoaderPort;
  onReloadFailure?: (error: unknown) => void;
  diagnose?: (message: string) => void;
}

const NOOP_MOTION: PetMotionHandle = { cancel() {} };

export class Live2DRenderer implements PetRenderer {
  private readonly loader: CubismModelLoaderPort;
  private status: Live2DRendererStatus = "unloaded";
  private visible = false;
  private generation = 0;
  private restoreAttempted = false;
  private asset: Live2DAsset | null = null;
  private model: LoadedCubismModel | null = null;
  private motions: MotionController | null = null;
  private mixer: ParameterMixer | null = null;
  private hitAreas: HitAreaResolver | null = null;
  private lookTarget: { x: number; y: number } | null = null;
  private lipSync: number | null = null;
  private viewport: { width: number; height: number; dpr: number } | null = null;

  private readonly onContextLost = (event: Event): void => {
    event.preventDefault();
    if (this.status === "destroyed" || this.status === "context-lost") return;
    ++this.generation;
    this.status = "context-lost";
    this.restoreAttempted = false;
  };

  private readonly onContextRestored = (): void => {
    if (this.status !== "context-lost" || this.restoreAttempted || !this.asset) return;
    this.restoreAttempted = true;
    void this.reloadAfterContextRestore(this.asset);
  };

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly options: Live2DRendererOptions = {},
  ) {
    this.loader = options.loader ?? new CubismModelLoader();
    canvas.addEventListener("webglcontextlost", this.onContextLost);
    canvas.addEventListener("webglcontextrestored", this.onContextRestored);
  }

  state(): Live2DRendererState {
    return { status: this.status, visible: this.visible };
  }

  async load(asset: PetRenderAsset): Promise<void> {
    if (!isLive2DRenderAsset(asset)) throw new Error("Live2DRenderer only accepts live2d assets");
    if (this.status === "destroyed") throw new Error("Live2DRenderer has been destroyed");

    const generation = ++this.generation;
    this.releaseCurrent();
    this.asset = asset;
    this.status = "loading";
    this.restoreAttempted = false;

    try {
      const model = await this.loader.load(this.canvas, asset.modelUrl);
      if (generation !== this.generation || this.isDestroyed() || this.asset !== asset) {
        model.release();
        return;
      }
      this.attachModel(model, asset);
    } catch (error) {
      if (generation === this.generation && this.asset === asset && !this.isDestroyed()) {
        this.asset = null;
        asset.dispose();
        this.status = "unloaded";
      }
      throw error;
    }
  }

  resize(viewport: { width: number; height: number; dpr: number }): void {
    this.viewport = {
      width: Math.max(1, viewport.width),
      height: Math.max(1, viewport.height),
      dpr: Math.max(1, viewport.dpr),
    };
    this.canvas.style.width = `${this.viewport.width}px`;
    this.canvas.style.height = `${this.viewport.height}px`;
    this.model?.resize(this.viewport.width, this.viewport.height, this.viewport.dpr);
  }

  playMotion(motion: PetMotion, options: { loop?: boolean; priority?: number } = {}): PetMotionHandle {
    if (this.status !== "ready" || !this.motions || !this.asset) return NOOP_MOTION;
    if (!this.asset.semantics.motions[motion]) {
      this.options.diagnose?.(`Live2D motion mapping is missing: ${motion}`);
      return NOOP_MOTION;
    }
    return this.motions.play(motion, options);
  }

  setExpression(expression: PetExpression, weight = 1): void {
    if (this.status !== "ready" || !this.asset || !this.model) return;
    const name = this.asset.semantics.expressions[expression];
    if (!name) {
      this.options.diagnose?.(`Live2D expression mapping is missing: ${expression}`);
      return;
    }
    this.model.setExpression(name, Math.min(1, Math.max(0, weight)));
  }

  setLookTarget(target: { x: number; y: number } | null): void {
    this.lookTarget = target
      ? { x: Math.min(1, Math.max(-1, target.x)), y: Math.min(1, Math.max(-1, target.y)) }
      : null;
  }

  setLipSync(value: number): void {
    this.lipSync = Math.min(1, Math.max(0, value));
  }

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    if (this.status !== "ready") return null;
    return this.hitAreas?.resolve(point) ?? null;
  }

  setVisibility(visible: boolean): void {
    this.visible = visible && this.status !== "destroyed";
    this.canvas.style.visibility = this.visible ? "visible" : "hidden";
  }

  update(deltaMs: number): void {
    if (this.status !== "ready" || !this.visible || !this.model) return;
    this.mixer?.apply({
      ...(this.lookTarget
        ? {
            lookX: this.lookTarget.x,
            lookY: this.lookTarget.y,
            angleX: this.lookTarget.x,
            angleY: this.lookTarget.y,
          }
        : {}),
      ...(this.lipSync === null ? {} : { lipSync: this.lipSync }),
    });
    this.model.update(Math.max(0, deltaMs));
    this.model.draw();
  }

  destroy(): void {
    if (this.status === "destroyed") return;
    ++this.generation;
    this.status = "destroyed";
    this.visible = false;
    this.releaseCurrent();
    this.canvas.style.visibility = "hidden";
    this.canvas.removeEventListener("webglcontextlost", this.onContextLost);
    this.canvas.removeEventListener("webglcontextrestored", this.onContextRestored);
  }

  private attachModel(model: LoadedCubismModel, asset: Live2DAsset): void {
    this.model = model;
    this.motions = new MotionController({
      port: {
        start: (motion, options, onFinished) => {
          const mapping = asset.semantics.motions[motion];
          if (!mapping) {
            this.options.diagnose?.(`Live2D motion mapping is missing: ${motion}`);
            return NOOP_MOTION;
          }
          return model.playMotion(mapping.group, mapping.index ?? 0, options, onFinished);
        },
        stopAll: () => model.stopAllMotions(),
      },
    });
    this.mixer = new ParameterMixer({
      semantics: asset.semantics.parameters,
      port: model,
      diagnose: (semantic) => this.options.diagnose?.(`Live2D parameter mapping is missing: ${semantic}`),
    });
    this.hitAreas = new HitAreaResolver(asset.semantics.hitAreas, model);
    if (this.viewport) model.resize(this.viewport.width, this.viewport.height, this.viewport.dpr);
    this.status = "ready";
  }

  private releaseCurrent(options: { preserveAsset?: boolean } = {}): void {
    this.motions?.stopAll();
    this.motions = null;
    this.mixer = null;
    this.hitAreas = null;
    this.model?.release();
    this.model = null;
    if (!options.preserveAsset) {
      this.asset?.dispose();
      this.asset = null;
    }
  }

  private async reloadAfterContextRestore(asset: Live2DAsset): Promise<void> {
    const generation = ++this.generation;
    this.releaseCurrent({ preserveAsset: true });
    this.status = "loading";
    try {
      const model = await this.loader.load(this.canvas, asset.modelUrl);
      if (generation !== this.generation || this.isDestroyed() || this.asset !== asset) {
        model.release();
        return;
      }
      this.attachModel(model, asset);
    } catch (error) {
      if (generation === this.generation && !this.isDestroyed() && this.asset === asset) {
        this.status = "context-lost";
        this.options.onReloadFailure?.(error);
      }
    }
  }

  private isDestroyed(): boolean {
    return this.status === "destroyed";
  }
}
