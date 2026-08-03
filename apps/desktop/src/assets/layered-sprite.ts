import { Assets, Container, Sprite } from "pixi.js";

export interface LayeredAsset {
  bodyUrl: string;
  eyeOpenUrl: string;
  eyeClosedUrl: string;
  accentUrl?: string;
}

export interface Size {
  width: number;
  height: number;
}

export interface LayerLayout extends Size {
  x: number;
  y: number;
  scale: number;
}

export function computeLayerLayout(source: Size, viewport: Size): LayerLayout {
  if (source.width <= 0 || source.height <= 0 || viewport.width <= 0 || viewport.height <= 0) {
    throw new RangeError("sizes must be positive");
  }
  const scale = Math.min(viewport.width / source.width, viewport.height / source.height);
  const width = source.width * scale;
  const height = source.height * scale;
  return {
    x: (viewport.width - width) / 2,
    y: viewport.height - height,
    width,
    height,
    scale,
  };
}

export function flipScaleX(scale: number, flipped: boolean): number {
  return flipped ? -Math.abs(scale) : Math.abs(scale);
}

export class LayeredSprite {
  private readonly container = new Container();
  private body!: Sprite;
  private eyeOpen!: Sprite;
  private eyeClosed!: Sprite;
  private accent: Sprite | undefined;
  private baseScale = 1;
  private breathPhase = 0;
  private carried = false;

  get stageObject(): Container {
    return this.container;
  }

  async mount(stage: Container, viewport: Size, flipped = false): Promise<void> {
    const textures = {
      body: await Assets.load(this.assets.bodyUrl),
      eyeOpen: await Assets.load(this.assets.eyeOpenUrl),
      eyeClosed: await Assets.load(this.assets.eyeClosedUrl),
      accent: this.assets.accentUrl ? await Assets.load(this.assets.accentUrl) : undefined,
    };
    const layout = computeLayerLayout(
      { width: textures.body.width, height: textures.body.height },
      viewport,
    );
    this.baseScale = layout.scale;

    this.body = new Sprite(textures.body);
    this.eyeOpen = new Sprite(textures.eyeOpen);
    this.eyeClosed = new Sprite(textures.eyeClosed);
    this.eyeClosed.visible = false;
    if (textures.accent) {
      this.accent = new Sprite(textures.accent);
    }

    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      layer.anchor.set(0, 0);
      layer.position.set(layout.x, layout.y);
      layer.scale.set(layout.scale);
    }
    this.container.addChild(this.body, this.eyeOpen, this.eyeClosed);
    if (this.accent) this.container.addChild(this.accent);
    this.setFlip(flipped);
    stage.addChild(this.container);
  }

  constructor(private readonly assets: LayeredAsset) {}

  setEyesOpen(open: boolean): void {
    this.eyeOpen.visible = open;
    this.eyeClosed.visible = !open;
  }

  setBreathPhase(phase: number): void {
    this.breathPhase = phase;
    const breathe = 1 + Math.sin(phase * Math.PI * 2) * 0.02;
    const scale = this.baseScale * breathe;
    const carriedScale = this.carried ? 0.94 : 1;
    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      layer.scale.set(flipScaleX(scale * carriedScale, this.container.scale.x < 0), scale * carriedScale);
    }
  }

  setFlip(flipped: boolean): void {
    this.container.scale.x = flipped ? -1 : 1;
  }

  setUserScale(scale: number): void {
    const current = this.container.scale.x < 0 ? -1 : 1;
    this.container.scale.set(current * scale, scale);
  }

  setSquash(factor: number): void {
    this.container.scale.y = this.baseScale * factor;
    this.container.scale.x = (this.container.scale.x < 0 ? -1 : 1) * this.baseScale / factor;
  }

  setShift(dx: number, dy: number): void {
    this.container.position.set(dx, dy);
  }

  setCarried(carried: boolean): void {
    this.carried = carried;
    const lift = carried ? 14 : 0;
    this.container.y = carried ? -lift : 0;
  }

  setAccentVisible(visible: boolean): void {
    if (this.accent) this.accent.visible = visible;
  }
}
