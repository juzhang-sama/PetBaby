import {
  isLive2DRenderAsset,
  type Live2DParameterSemantic,
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
import { MicroMotionController } from "./micro-motion";
import { MotionController } from "./motion-controller";
import { ParameterMixer } from "./parameter-mixer";
import type { ParameterValues } from "./parameter-mixer";
import type { CatAutomationFrame, CatMotionNameV1 } from "./cat-motion-contract";
import { CatAutomationController, type CatAutomationMode } from "./cat-automation";
import { CatBlinkOverlay, type CatBlinkOverlayPort } from "./cat-blink-overlay";
import {
  canonicalPetCalibration,
  DEFAULT_PET_CALIBRATION,
  type PetCalibrationV1,
} from "../runtime/pet-calibration";

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
  catAutomationRandom?: () => number;
  createCatBlinkOverlay?: (canvas: HTMLCanvasElement, imageUrl: string) => CatBlinkOverlayPort;
}

const NOOP_MOTION: PetMotionHandle = { cancel() {} };

export class Live2DRenderer implements PetRenderer {
  private readonly loader: CubismModelLoaderPort;
  private readonly microMotion = new MicroMotionController();
  private readonly catAutomationController: CatAutomationController;
  private status: Live2DRendererStatus = "unloaded";
  private visible = false;
  private generation = 0;
  private restoreAttempted = false;
  private asset: Live2DAsset | null = null;
  private model: LoadedCubismModel | null = null;
  private motions: MotionController | null = null;
  private mixer: ParameterMixer | null = null;
  private hitAreas: HitAreaResolver | null = null;
  private blinkOverlay: CatBlinkOverlayPort | null = null;
  private backgroundMotion: { name: PetMotion; priority: number; loop: true } | null = null;
  private interruptedMotion: {
    name: PetMotion;
    priority: number;
    loop: boolean;
    fadeInMs?: number;
    fadeOutMs?: number;
  } | null = null;
  private lipSync: number | null = null;
  private catAutomationOverride: ParameterValues | null = null;
  private catPointerInteraction: ParameterValues | null = null;
  private catAutomationMode: CatAutomationMode = "paused";
  private viewport: { width: number; height: number; dpr: number } | null = null;
  private calibration: PetCalibrationV1 = { ...DEFAULT_PET_CALIBRATION };

  private readonly onContextLost = (event: Event): void => {
    event.preventDefault();
    if (this.status === "destroyed" || this.status === "context-lost") return;
    const currentMotion = this.motions?.current();
    this.interruptedMotion = currentMotion === null || currentMotion === undefined
      ? null
      : {
          name: currentMotion.name,
          priority: currentMotion.priority,
          loop: currentMotion.loop,
          ...(currentMotion.fadeInMs === undefined ? {} : { fadeInMs: currentMotion.fadeInMs }),
          ...(currentMotion.fadeOutMs === undefined ? {} : { fadeOutMs: currentMotion.fadeOutMs }),
        };
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
    this.catAutomationController = new CatAutomationController({ random: options.catAutomationRandom });
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
    this.interruptedMotion = null;
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
    if (motion === "carried") this.microMotion.setCarried(true);
    if (motion === "landed" || motion === "idle" || motion === "wake") {
      this.microMotion.setCarried(false);
    }
    if (!this.asset.semantics.motions[motion]) {
      this.options.diagnose?.(`Live2D motion mapping is missing: ${motion}`);
      return NOOP_MOTION;
    }
    if (options.loop && (options.priority ?? 0) < 80) {
      this.backgroundMotion = { name: motion, priority: options.priority ?? 0, loop: true };
    }
    return this.motions.play(motion, options);
  }

  playCatMotion(
    motion: CatMotionNameV1,
    transition: {
      loop?: boolean;
      priority?: number;
      fadeInMs?: number;
      fadeOutMs?: number;
    } = {},
    onFinished?: () => void,
  ): PetMotionHandle {
    if (!this.supportsCatMotionV1() || !this.motions || !this.asset?.semantics.motions[motion]) {
      return NOOP_MOTION;
    }
    return this.motions.play(motion, transition, onFinished);
  }

  supportsCatMotionV1(): boolean {
    return this.asset?.catV4 === true;
  }

  setCatAutomation(frame: CatAutomationFrame | null): void {
    this.catAutomationOverride = frame === null ? null : {
      bodyBreath: frame.breath,
      eyeOpenLeft: frame.eyeLeftOpen,
      eyeOpenRight: frame.eyeRightOpen,
      earLeft: frame.earLeft,
      earRight: frame.earRight,
      tailAngle: frame.tailAngle,
      tailCurl: frame.tailCurl,
      tailTip: frame.tailTip,
    };
  }

  setCatAutomationMode(mode: CatAutomationMode): void {
    this.catAutomationMode = this.supportsCatMotionV1() ? mode : "paused";
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
    if (!this.supportsCatMotionV1() || target === null) {
      this.catPointerInteraction = null;
      return;
    }
    const x = Math.min(1, Math.max(-1, target.x));
    const y = Math.min(1, Math.max(-1, target.y));
    this.catPointerInteraction = {
      eyeBallX: x,
      eyeBallY: y,
      earLeft: Math.min(0.35, Math.max(-0.35, -0.15 * y - 0.1 * x)),
      earRight: Math.min(0.35, Math.max(-0.35, -0.15 * y + 0.1 * x)),
      tailAngle: x * 6,
      tailCurl: x * y * 0.15,
      tailTip: y * 0.25,
    };
  }

  setLipSync(value: number): void {
    this.lipSync = Math.min(1, Math.max(0, value));
  }

  setCalibration(value: PetCalibrationV1): void {
    if (this.isDestroyed()) throw new Error("Live2DRenderer has been destroyed");
    this.calibration = canonicalPetCalibration(value);
  }

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    if (this.status !== "ready") return null;
    return this.hitAreas?.resolve(point) ?? null;
  }

  setVisibility(visible: boolean): void {
    this.visible = visible && this.status !== "destroyed";
    this.microMotion.setPaused(!this.visible);
    this.canvas.style.visibility = this.visible ? "visible" : "hidden";
    this.blinkOverlay?.setVisible(this.visible);
  }

  update(deltaMs: number): void {
    if (this.status !== "ready" || !this.visible || !this.model) return;
    const frame = this.microMotion.update(Math.max(0, deltaMs));
    const automatedFrame = this.catAutomationMode === "paused"
      ? null
      : this.catAutomationController.update(Math.max(0, deltaMs), this.catAutomationMode);
    const automation = automatedFrame === null ? undefined : automationValues(automatedFrame);
    const interaction = this.asset?.catV4
      ? { eyeBallX: 0, eyeBallY: 0, ...this.catPointerInteraction, ...this.catAutomationOverride }
      : undefined;
    this.mixer?.apply({
      ...(this.asset?.catV4
        ? {
            ...(automation === undefined ? {} : { automation }),
            ...(interaction === undefined ? {} : { interaction }),
          }
        : { breath: 0.5 + frame.breath * this.calibration.breathAmplitudePercent / 100 }),
      sway: frame.bodySway,
      ...(this.lipSync === null ? {} : { lipSync: this.lipSync }),
    });
    this.blinkOverlay?.setEyesOpen(
      interaction?.eyeOpenLeft ?? automation?.eyeOpenLeft ?? 1,
      interaction?.eyeOpenRight ?? automation?.eyeOpenRight ?? 1,
    );
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

  private attachModel(
    model: LoadedCubismModel,
    asset: Live2DAsset,
    resumeMotion: {
      name: PetMotion;
      priority: number;
      loop: boolean;
      fadeInMs?: number;
      fadeOutMs?: number;
    } | null = null,
  ): void {
    const modelGeneration = this.generation;
    this.model = model;
    this.motions = new MotionController({
      port: {
        start: (motion, options, onFinished) => {
          const mapping = asset.semantics.motions[motion];
          if (!mapping) {
            this.options.diagnose?.(`Live2D motion mapping is missing: ${motion}`);
            return NOOP_MOTION;
          }
          return model.playMotion(mapping.group, mapping.index ?? 0, options, () => {
            if (modelGeneration !== this.generation || this.model !== model) return;
            onFinished();
          });
        },
        stopAll: () => model.stopAllMotions(),
      },
      resumeForState: () => this.backgroundMotion,
    });
    this.mixer = new ParameterMixer({
      semantics: asset.semantics.parameters,
      ...(asset.motionSpatialProfile === undefined
        ? {}
        : { motionSpatialProfile: asset.motionSpatialProfile }),
      port: model,
      diagnose: (semantic, issue) => issue === "incompatible-range"
        ? this.diagnoseIncompatibleParameterRange(semantic)
        : this.diagnoseMissingParameter(semantic),
      silentMissing: new Set(["bodyBreath", "bodySway"]),
    });
    this.hitAreas = new HitAreaResolver(asset.semantics.hitAreas, model);
    if (asset.catV4 && asset.blinkOverlayUrl) {
      const createBlinkOverlay = this.options.createCatBlinkOverlay
        ?? ((canvas, imageUrl) => new CatBlinkOverlay(canvas, imageUrl));
      this.blinkOverlay = createBlinkOverlay(this.canvas, asset.blinkOverlayUrl);
      this.blinkOverlay.setVisible(this.visible);
    }
    if (this.viewport) model.resize(this.viewport.width, this.viewport.height, this.viewport.dpr);
    this.status = "ready";
    const motionToResume = resumeMotion ?? this.backgroundMotion;
    if (motionToResume) {
      this.motions.play(motionToResume.name, motionToResume);
    }
  }

  private releaseCurrent(options: { preserveAsset?: boolean } = {}): void {
    this.motions?.stopAll();
    this.motions = null;
    this.mixer = null;
    this.hitAreas = null;
    this.blinkOverlay?.destroy();
    this.blinkOverlay = null;
    this.model?.release();
    this.model = null;
    if (!options.preserveAsset) {
      this.asset?.dispose();
      this.asset = null;
    }
  }

  private async reloadAfterContextRestore(asset: Live2DAsset): Promise<void> {
    const generation = ++this.generation;
    const resumeMotion = this.interruptedMotion;
    this.interruptedMotion = null;
    this.releaseCurrent({ preserveAsset: true });
    this.status = "loading";
    try {
      const model = await this.loader.load(this.canvas, asset.modelUrl);
      if (generation !== this.generation || this.isDestroyed() || this.asset !== asset) {
        model.release();
        return;
      }
      this.attachModel(model, asset, resumeMotion);
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

  private diagnoseMissingParameter(semantic: Live2DParameterSemantic): void {
    this.options.diagnose?.(`Live2D parameter mapping is missing: ${semantic}`);
  }

  private diagnoseIncompatibleParameterRange(semantic: Live2DParameterSemantic): void {
    this.options.diagnose?.(`Live2D model/profile parameter ranges are incompatible: ${semantic}`);
  }
}

function automationValues(frame: CatAutomationFrame): ParameterValues {
  return {
    bodyBreath: frame.breath,
    eyeOpenLeft: frame.eyeLeftOpen,
    eyeOpenRight: frame.eyeRightOpen,
    earLeft: frame.earLeft,
    earRight: frame.earRight,
    tailAngle: frame.tailAngle,
    tailCurl: frame.tailCurl,
    tailTip: frame.tailTip,
  };
}
