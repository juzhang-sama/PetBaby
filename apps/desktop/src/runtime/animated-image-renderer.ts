import type { MotionProfileV1 } from "./animated-image-manifest";
import {
  LIFE_V1,
  computeMotionFrame,
  planBreathSlices,
  type BreathSlice,
} from "./animated-image-motion";
import { computeContainRect, type LayoutRect } from "./geometry";
import type {
  PetExpression,
  PetHitArea,
  PetMotion,
  PetMotionHandle,
  PetRenderAsset,
  PetRenderer,
} from "./pet-renderer";
import { loadBrowserImage } from "./static-png-renderer";

type AnimatedImage = CanvasImageSource & { width: number; height: number };
type BreathPlanner = (
  profile: MotionProfileV1,
  imageWidth: number,
  imageHeight: number,
  breath: number,
  sliceCount: number,
) => BreathSlice[];

export interface AnimatedImageRendererOptions {
  createCanvas?: () => HTMLCanvasElement;
  loadImage?: (url: string) => Promise<AnimatedImage>;
  planBreathSlices?: BreathPlanner;
}

interface Viewport {
  width: number;
  height: number;
  dpr: number;
}

export class AnimatedImageRenderer implements PetRenderer {
  private readonly displayCanvas: HTMLCanvasElement;
  private readonly hitCanvas: HTMLCanvasElement;
  private readonly displayContext: CanvasRenderingContext2D;
  private readonly hitContext: CanvasRenderingContext2D;
  private readonly loadImage: (url: string) => Promise<AnimatedImage>;
  private readonly makeBreathSlices: BreathPlanner;
  private image: AnimatedImage | undefined;
  private profile: MotionProfileV1 | undefined;
  private viewport: Viewport | undefined;
  private bounds: LayoutRect | undefined;
  private elapsedMs = 0;
  private idle = false;
  private visible = false;
  private destroyed = false;
  private loadToken = 0;

  constructor(
    private readonly root: HTMLElement,
    options: AnimatedImageRendererOptions = {},
  ) {
    const createCanvas = options.createCanvas ?? (() => document.createElement("canvas"));
    this.displayCanvas = createCanvas();
    this.hitCanvas = createCanvas();
    const displayContext = this.displayCanvas.getContext("2d");
    const hitContext = this.hitCanvas.getContext("2d");
    if (!displayContext || !hitContext) {
      throw new Error("2D canvas is unavailable for animated image rendering");
    }
    this.displayContext = displayContext;
    this.hitContext = hitContext;
    this.loadImage = options.loadImage ?? loadBrowserImage;
    this.makeBreathSlices = options.planBreathSlices ?? planBreathSlices;
    this.displayCanvas.style.display = "block";
    this.hitCanvas.style.display = "none";
    this.displayCanvas.style.visibility = "hidden";
    this.hitCanvas.style.visibility = "hidden";
  }

  async load(asset: PetRenderAsset): Promise<void> {
    this.assertAlive();
    if (asset.kind !== "animated-image") {
      throw new TypeError("AnimatedImageRenderer only accepts animated-image assets");
    }
    const loadToken = ++this.loadToken;
    let image: AnimatedImage;
    try {
      image = await this.loadImage(asset.imageUrl);
    } catch (error) {
      if (this.destroyed || loadToken !== this.loadToken) return;
      throw error;
    }
    if (this.destroyed || loadToken !== this.loadToken) return;
    this.image = image;
    this.profile = asset.motionProfile;
    this.elapsedMs = 0;
    this.idle = false;
    this.root.replaceChildren(this.displayCanvas, this.hitCanvas);
    this.recomputeLayout();
    this.renderDisplay();
    this.renderHitEnvelope();
  }

  resize(viewport: Viewport): void {
    this.assertAlive();
    if (viewport.width <= 0 || viewport.height <= 0 || viewport.dpr <= 0) {
      throw new RangeError("viewport dimensions and dpr must be positive");
    }
    this.viewport = { ...viewport };
    for (const canvas of [this.displayCanvas, this.hitCanvas]) {
      canvas.width = Math.max(1, Math.round(viewport.width * viewport.dpr));
      canvas.height = Math.max(1, Math.round(viewport.height * viewport.dpr));
      canvas.style.width = `${viewport.width}px`;
      canvas.style.height = `${viewport.height}px`;
    }
    this.displayContext.setTransform(viewport.dpr, 0, 0, viewport.dpr, 0, 0);
    this.hitContext.setTransform(viewport.dpr, 0, 0, viewport.dpr, 0, 0);
    this.recomputeLayout();
    this.renderDisplay();
    this.renderHitEnvelope();
  }

  playMotion(motion: PetMotion, _options?: { loop?: boolean; priority?: number }): PetMotionHandle {
    if (motion !== "idle" || this.destroyed) return { cancel: () => undefined };
    this.idle = true;
    let active = true;
    return {
      cancel: () => {
        if (!active) return;
        active = false;
        this.idle = false;
      },
    };
  }

  setExpression(_expression: PetExpression, _weight?: number): void {}

  setLookTarget(_target: { x: number; y: number } | null): void {}

  setLipSync(_value: number): void {}

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    if (!this.image || !this.profile || !this.bounds || !this.visible || this.destroyed) return null;
    const alpha = this.profile.alphaBounds;
    const left = this.bounds.x + alpha.left * this.bounds.width;
    const top = this.bounds.y + alpha.top * this.bounds.height;
    const right = this.bounds.x + alpha.right * this.bounds.width;
    const bottom = this.bounds.y + alpha.bottom * this.bounds.height;
    return point.x >= left && point.x < right && point.y >= top && point.y < bottom ? "body" : null;
  }

  setVisibility(visible: boolean): void {
    if (this.destroyed) return;
    this.visible = visible;
    const visibility = visible ? "visible" : "hidden";
    this.displayCanvas.style.visibility = visibility;
    this.hitCanvas.style.visibility = visibility;
  }

  update(deltaMs: number): void {
    if (!this.idle || this.destroyed || !Number.isFinite(deltaMs) || deltaMs < 0) return;
    this.elapsedMs += deltaMs;
    this.renderDisplay();
  }

  getHitSurface(): HTMLCanvasElement {
    return this.hitCanvas;
  }

  destroy(): void {
    if (this.destroyed) return;
    this.loadToken += 1;
    const clearWidth = this.viewport?.width ?? this.displayCanvas.width;
    const clearHeight = this.viewport?.height ?? this.displayCanvas.height;
    this.displayContext.clearRect(0, 0, clearWidth, clearHeight);
    this.hitContext.clearRect(0, 0, clearWidth, clearHeight);
    this.displayCanvas.style.visibility = "hidden";
    this.hitCanvas.style.visibility = "hidden";
    this.displayCanvas.remove();
    this.hitCanvas.remove();
    this.image = undefined;
    this.profile = undefined;
    this.bounds = undefined;
    this.idle = false;
    this.visible = false;
    this.destroyed = true;
  }

  private recomputeLayout(): void {
    if (!this.image || !this.viewport) {
      this.bounds = undefined;
      return;
    }
    this.bounds = computeContainRect(this.image, this.viewport);
  }

  private renderDisplay(): void {
    if (!this.image || !this.profile || !this.viewport || !this.bounds || this.destroyed) return;
    const frame = computeMotionFrame(this.elapsedMs);
    const slices = this.makeBreathSlices(
      this.profile,
      this.image.width,
      this.image.height,
      frame.breath,
      24,
    );
    const pivotX = this.bounds.x + this.profile.swayPivot.x * this.bounds.width;
    const pivotY = this.bounds.y + this.profile.swayPivot.y * this.bounds.height;
    const shiftX = this.viewport.width * frame.swayXRatio;
    this.displayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.displayContext.save();
    this.displayContext.translate(pivotX + shiftX, pivotY);
    this.displayContext.rotate(frame.swayRadians);
    this.displayContext.translate(-pivotX, -pivotY);
    for (const slice of slices) {
      this.displayContext.drawImage(
        this.image,
        slice.sourceX,
        slice.sourceY,
        slice.sourceWidth,
        slice.sourceHeight,
        this.bounds.x + slice.destX * this.bounds.scale,
        this.bounds.y + slice.destY * this.bounds.scale,
        slice.destWidth * this.bounds.scale,
        slice.destHeight * this.bounds.scale,
      );
    }
    this.displayContext.restore();
  }

  private renderHitEnvelope(): void {
    if (!this.image || !this.profile || !this.viewport || !this.bounds || this.destroyed) return;
    const pivotX = this.bounds.x + this.profile.swayPivot.x * this.bounds.width;
    const pivotY = this.bounds.y + this.profile.swayPivot.y * this.bounds.height;
    const maxShiftX = this.viewport.width * LIFE_V1.swayXRatio;
    const poses = [
      { shiftX: 0, radians: 0 },
      { shiftX: -maxShiftX, radians: 0 },
      { shiftX: maxShiftX, radians: 0 },
      { shiftX: 0, radians: -LIFE_V1.swayRadians },
      { shiftX: 0, radians: LIFE_V1.swayRadians },
    ];
    this.hitContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    for (const pose of poses) {
      this.hitContext.save();
      this.hitContext.translate(pivotX + pose.shiftX, pivotY);
      this.hitContext.rotate(pose.radians);
      this.hitContext.translate(-pivotX, -pivotY);
      this.hitContext.drawImage(
        this.image,
        this.bounds.x,
        this.bounds.y,
        this.bounds.width,
        this.bounds.height,
      );
      this.hitContext.restore();
    }
  }

  private assertAlive(): void {
    if (this.destroyed) throw new Error("AnimatedImageRenderer has been destroyed");
  }
}
