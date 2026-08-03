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

export interface BodyBounds {
  x: number;
  y: number;
  width: number;
  height: number;
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

export class LayeredSprite {
  private readonly container = new Container();
  private body!: Sprite;
  private eyeOpen!: Sprite;
  private eyeClosed!: Sprite;
  private accent: Sprite | undefined;
  private baseScale = 1;
  private userScale = 1;
  private flipped = false;
  private breathPhase = 0;
  private carried = false;
  private layout!: LayerLayout;

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
    this.layout = computeLayerLayout(
      { width: textures.body.width, height: textures.body.height },
      viewport,
    );
    this.baseScale = this.layout.scale;

    this.body = new Sprite(textures.body);
    this.eyeOpen = new Sprite(textures.eyeOpen);
    this.eyeClosed = new Sprite(textures.eyeClosed);
    this.eyeClosed.visible = false;
    if (textures.accent) {
      this.accent = new Sprite(textures.accent);
    }

    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      layer.anchor.set(0.5, 0);
      layer.position.set(this.layout.x + this.layout.width / 2, this.layout.y);
      layer.scale.set(this.layout.scale);
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
    const carriedScale = this.carried ? 0.94 : 1;
    const scale = this.baseScale * breathe * carriedScale;
    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      const sx = this.flipped ? -scale : scale;
      layer.scale.set(sx, scale);
    }
  }

  setFlip(flipped: boolean): void {
    this.flipped = flipped;
  }

  setUserScale(scale: number): void {
    this.userScale = scale;
    this.container.scale.set(scale, scale);
  }

  setSquash(factor: number): void {
    const breathe = 1 + Math.sin(this.breathPhase * Math.PI * 2) * 0.02;
    const base = this.baseScale * breathe * (this.carried ? 0.94 : 1);
    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      const sx = this.flipped ? -(base / factor) : base / factor;
      layer.scale.set(sx, base * factor);
    }
  }

  setShift(dx: number, dy: number): void {
    this.container.position.set(dx, dy);
  }

  setCarried(carried: boolean): void {
    this.carried = carried;
  }

  setAccentVisible(visible: boolean): void {
    if (this.accent) this.accent.visible = visible;
  }

  /** Current visual bounds of the body layer in viewport coordinates. */
  getBodyBounds(): BodyBounds {
    const body = this.body;
    const cs = this.container.scale;
    const width = body.width * Math.abs(body.scale.x) * Math.abs(cs.x);
    const height = body.height * Math.abs(body.scale.y) * Math.abs(cs.y);
    const centerX = (body.x + this.container.position.x) * cs.x;
    const topY = (body.y + this.container.position.y) * cs.y;
    return {
      x: centerX - width / 2,
      y: topY,
      width,
      height,
    };
  }
}
