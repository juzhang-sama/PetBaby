import { computeContainRect, type LayoutRect } from "./geometry";
import type {
  PetExpression,
  PetHitArea,
  PetMotion,
  PetMotionHandle,
  PetRenderAsset,
  PetRenderer,
} from "./pet-renderer";

type StaticPngImage = CanvasImageSource & { width: number; height: number };

export interface StaticPngRendererOptions {
  createCanvas?: () => HTMLCanvasElement;
  loadImage?: (url: string) => Promise<StaticPngImage>;
}

export interface StaticPngRendererState {
  loaded: boolean;
  visible: boolean;
  destroyed: boolean;
}

interface Viewport {
  width: number;
  height: number;
  dpr: number;
}

async function loadBrowserImage(url: string): Promise<StaticPngImage> {
  const image = new Image();
  image.src = url;
  await image.decode();
  return image;
}

export class StaticPngRenderer implements PetRenderer {
  private readonly canvas: HTMLCanvasElement;
  private readonly context: CanvasRenderingContext2D;
  private readonly loadImage: (url: string) => Promise<StaticPngImage>;
  private image: StaticPngImage | undefined;
  private viewport: Viewport | undefined;
  private bounds: LayoutRect | undefined;
  private loaded = false;
  private visible = false;
  private destroyed = false;
  private loadToken = 0;

  constructor(
    private readonly root: HTMLElement,
    options: StaticPngRendererOptions = {},
  ) {
    this.canvas = options.createCanvas?.() ?? document.createElement("canvas");
    const context = this.canvas.getContext("2d");
    if (!context) throw new Error("2D canvas is unavailable for static PNG rendering");
    this.context = context;
    this.loadImage = options.loadImage ?? loadBrowserImage;
    this.canvas.style.display = "block";
    this.canvas.style.visibility = "hidden";
  }

  async load(asset: PetRenderAsset): Promise<void> {
    this.assertAlive();
    if (asset.kind !== "static-png") {
      throw new TypeError("StaticPngRenderer only accepts static-png assets");
    }
    const loadToken = ++this.loadToken;
    let image: StaticPngImage;
    try {
      image = await this.loadImage(asset.imageUrl);
    } catch (error) {
      if (this.destroyed || loadToken !== this.loadToken) return;
      throw error;
    }
    if (this.destroyed || loadToken !== this.loadToken) return;
    this.image = image;
    this.loaded = true;
    this.root.replaceChildren(this.canvas);
    this.render();
  }

  resize(viewport: Viewport): void {
    this.assertAlive();
    if (viewport.width <= 0 || viewport.height <= 0 || viewport.dpr <= 0) {
      throw new RangeError("viewport dimensions and dpr must be positive");
    }
    this.viewport = { ...viewport };
    this.canvas.width = Math.max(1, Math.round(viewport.width * viewport.dpr));
    this.canvas.height = Math.max(1, Math.round(viewport.height * viewport.dpr));
    this.canvas.style.width = `${viewport.width}px`;
    this.canvas.style.height = `${viewport.height}px`;
    this.context.setTransform(viewport.dpr, 0, 0, viewport.dpr, 0, 0);
    this.render();
  }

  playMotion(_motion: PetMotion, _options?: { loop?: boolean; priority?: number }): PetMotionHandle {
    return { cancel: () => undefined };
  }

  setExpression(_expression: PetExpression, _weight?: number): void {}

  setLookTarget(_target: { x: number; y: number } | null): void {}

  setLipSync(_value: number): void {}

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    if (!this.loaded || !this.visible || this.destroyed || !this.bounds) return null;
    const inside = point.x >= this.bounds.x
      && point.x < this.bounds.x + this.bounds.width
      && point.y >= this.bounds.y
      && point.y < this.bounds.y + this.bounds.height;
    return inside ? "body" : null;
  }

  setVisibility(visible: boolean): void {
    if (this.destroyed) return;
    this.visible = visible;
    this.canvas.style.visibility = visible ? "visible" : "hidden";
  }

  update(_deltaMs: number): void {}

  destroy(): void {
    if (this.destroyed) return;
    this.loadToken += 1;
    if (this.viewport) this.context.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.canvas.remove();
    this.image = undefined;
    this.bounds = undefined;
    this.loaded = false;
    this.visible = false;
    this.destroyed = true;
    this.canvas.style.visibility = "hidden";
  }

  state(): StaticPngRendererState {
    return { loaded: this.loaded, visible: this.visible, destroyed: this.destroyed };
  }

  private render(): void {
    if (!this.image || !this.viewport || this.destroyed) return;
    this.context.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.bounds = computeContainRect(
      { width: this.image.width, height: this.image.height },
      { width: this.viewport.width, height: this.viewport.height },
    );
    this.context.drawImage(
      this.image,
      this.bounds.x,
      this.bounds.y,
      this.bounds.width,
      this.bounds.height,
    );
  }

  private assertAlive(): void {
    if (this.destroyed) throw new Error("StaticPngRenderer has been destroyed");
  }
}
