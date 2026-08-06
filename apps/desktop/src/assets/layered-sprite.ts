import { Assets, Container, Graphics, Matrix, Sprite } from "pixi.js";
import type { AffineTransform } from "../runtime/bone";
import type {
  ManifestFeatureBox,
  ManifestMeshFeatures,
  ManifestPart,
} from "../runtime/manifest-schema";
import { heuristicFeatures, type FeatureRects } from "../runtime/mesh-rig";
import { PetMesh } from "../runtime/pet-mesh";
import type { RiggedPart } from "../runtime/part-rig";

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

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface BodyBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Convert an affine transform into a Pixi Matrix (same a/b/c/d/tx/ty layout). */
export function toPixiMatrix(transform: AffineTransform): Matrix {
  return new Matrix(
    transform.a,
    transform.b,
    transform.c,
    transform.d,
    transform.tx,
    transform.ty,
  );
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

/**
 * Layout for an asset whose opaque subject occupies only part of the canvas.
 * The whole sprite is scaled so the SUBJECT fits the viewport, and the sprite
 * is shifted so the subject lands at the same position a subject-sized asset
 * would occupy (bottom-aligned, horizontally centered).
 */
export function computeSubjectLayerLayout(
  source: Size,
  subject: Rect,
  viewport: Size,
): LayerLayout {
  const fit = computeLayerLayout(
    { width: subject.width, height: subject.height },
    viewport,
  );
  return {
    x: fit.x - subject.x * fit.scale,
    y: fit.y - subject.y * fit.scale,
    width: source.width * fit.scale,
    height: source.height * fit.scale,
    scale: fit.scale,
  };
}

/**
 * Container offset that keeps the viewport bottom-center anchored while a
 * user scale is applied to the container (Pixi scales around the origin).
 *
 * Children are laid out in viewport coordinates; a child at position `p`
 * renders at `offset + p * scale`. Requiring the viewport bottom-center
 * `anchor = (width / 2, height)` to stay fixed gives
 * `offset = anchor * (1 - scale)`.
 */
export function computeAnchorPosition(viewport: Size, scale: number): { x: number; y: number } {
  if (scale <= 0) return { x: 0, y: 0 };
  return {
    x: (viewport.width / 2) * (1 - scale),
    y: viewport.height * (1 - scale),
  };
}

/** Bounding box of the opaque pixels (alpha >= 32) of an image URL. */
export async function readOpaqueBounds(url: string): Promise<Rect | null> {
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = img.naturalWidth;
    canvas.height = img.naturalHeight;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return null;
    ctx.drawImage(img, 0, 0);
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let minX = canvas.width;
    let minY = canvas.height;
    let maxX = 0;
    let maxY = 0;
    let found = false;
    for (let y = 0; y < canvas.height; y += 1) {
      for (let x = 0; x < canvas.width; x += 1) {
        if ((data[(y * canvas.width + x) * 4 + 3] ?? 0) >= 32) {
          found = true;
          if (x < minX) minX = x;
          if (x > maxX) maxX = x;
          if (y < minY) minY = y;
          if (y > maxY) maxY = y;
        }
      }
    }
    if (!found) return null;
    return { x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1 };
  } catch {
    return null;
  }
}

export class LayeredSprite {
  private readonly container = new Container();
  private source!: Size;
  private subject: Rect | null = null;
  private viewport!: Size;
  private anchorPosition = { x: 0, y: 0 };
  private body!: Sprite;
  private eyeOpen!: Sprite;
  private eyeClosed!: Sprite;
  private accent: Sprite | undefined;
  private petMesh: PetMesh | null = null;
  private readonly lids = new Graphics();
  private eyesOpen = true;
  private headTurn = 0;
  private baseScale = 1;
  private userScale = 1;
  private flipped = false;
  private breathPhase = 0;
  private carried = false;
  private layout!: LayerLayout;

  get stageObject(): Container {
    return this.container;
  }

  get currentLayout(): LayerLayout {
    return this.layout;
  }

  private get isSingleImage(): boolean {
    return this.assets.eyeOpenUrl === this.assets.bodyUrl
      && this.assets.eyeClosedUrl === this.assets.bodyUrl;
  }

  async mount(stage: Container, viewport: Size, flipped = false): Promise<void> {
    const textures = {
      body: await Assets.load(this.assets.bodyUrl),
      eyeOpen: await Assets.load(this.assets.eyeOpenUrl),
      eyeClosed: await Assets.load(this.assets.eyeClosedUrl),
      accent: this.assets.accentUrl ? await Assets.load(this.assets.accentUrl) : undefined,
    };
    this.source = { width: textures.body.width, height: textures.body.height };
    this.subject = await readOpaqueBounds(this.assets.bodyUrl);

    this.body = new Sprite(textures.body);
    this.eyeOpen = new Sprite(textures.eyeOpen);
    this.eyeClosed = new Sprite(textures.eyeClosed);
    this.eyeClosed.visible = false;
    if (textures.accent) {
      this.accent = new Sprite(textures.accent);
    }
    if (this.isSingleImage) {
      this.petMesh = new PetMesh(
        textures.body,
        this.subject ?? { x: 0, y: 0, width: this.source.width, height: this.source.height },
        this.featuresForMesh(),
      );
      this.body.visible = false;
      this.eyeOpen.visible = false;
      this.eyeClosed.visible = false;
    }

    this.container.addChild(this.body, this.eyeOpen, this.eyeClosed);
    if (this.accent) this.container.addChild(this.accent);
    if (this.petMesh) this.container.addChild(this.petMesh.mesh);
    this.container.addChild(this.lids);
    this.drawLids();
    this.relayout(viewport);
    this.setFlip(flipped);
    stage.addChild(this.container);
  }

  constructor(
    private readonly assets: LayeredAsset,
    private readonly parts: ManifestPart[] = [],
    private readonly meshFeatures?: ManifestMeshFeatures,
  ) {}

  /** Recompute the base layout for a (possibly changed) viewport. */
  relayout(viewport: Size): void {
    this.viewport = viewport;
    this.layout = this.subject
      && (this.subject.width < this.source.width || this.subject.height < this.source.height)
      ? computeSubjectLayerLayout(this.source, this.subject, viewport)
      : computeLayerLayout(this.source, viewport);
    this.baseScale = this.layout.scale;
    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      layer.anchor.set(0.5, 0);
      layer.position.set(this.layout.x + this.layout.width / 2, this.layout.y);
      layer.scale.set(this.layout.scale);
    }
    if (this.petMesh) {
      this.petMesh.mesh.position.set(this.layout.x + this.layout.width / 2, this.layout.y);
      this.petMesh.mesh.scale.set(
        this.flipped ? -this.layout.scale : this.layout.scale,
        this.layout.scale,
      );
    }
    this.lids.position.set(this.layout.x, this.layout.y);
    this.lids.scale.set(this.layout.scale);
  }

  setEyesOpen(open: boolean): void {
    this.eyesOpen = open;
    this.eyeOpen.visible = open && !this.isSingleImage;
    this.eyeClosed.visible = !open && !this.isSingleImage;
    this.lids.visible = !open && this.isSingleImage;
    this.applyMesh();
  }

  setHeadTurn(amount: number): void {
    this.headTurn = amount;
    this.applyMesh();
  }

  /**
   * Apply a computed part rig. Each rigged part's transform already includes
   * its pivot offset, so the sprite uses the default anchor (0,0) and the
   * matrix is decomposed directly into position/rotation/scale/skew.
   */
  applyRig(rig: RiggedPart[]): void {
    for (const rigged of rig) {
      const sprite = this.spriteByRole(rigged.role);
      if (!sprite) continue;
      sprite.anchor.set(0, 0);
      sprite.setFromMatrix(toPixiMatrix(rigged.transform));
    }
  }

  private spriteByRole(role: string): Sprite | undefined {
    if (role === "body") return this.body;
    if (role === "eye-open") return this.eyeOpen;
    if (role === "eye-closed") return this.eyeClosed;
    if (role === "accent") return this.accent;
    return undefined;
  }

  setBreathPhase(phase: number): void {
    this.breathPhase = phase;
    // subtle volume change: vertical stretch with a slight horizontal counter,
    // instead of scaling the whole sprite like a zoom
    const wave = Math.sin(phase * Math.PI * 2);
    const breatheY = 1 + wave * 0.012;
    const breatheX = 1 - wave * 0.006;
    const carriedScale = this.carried ? 0.94 : 1;
    const scaleY = this.baseScale * breatheY * carriedScale;
    const scaleX = this.baseScale * breatheX * carriedScale;
    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      const sx = this.flipped ? -scaleX : scaleX;
      layer.scale.set(sx, scaleY);
    }
    if (this.petMesh) {
      const sx = this.flipped ? -scaleX : scaleX;
      this.petMesh.mesh.scale.set(sx, scaleY);
    }
    this.applyMesh();
  }

  setFlip(flipped: boolean): void {
    this.flipped = flipped;
  }

  setUserScale(scale: number): void {
    this.userScale = scale;
    this.container.scale.set(scale, scale);
    this.anchorPosition = this.viewport
      ? computeAnchorPosition(this.viewport, scale)
      : { x: 0, y: 0 };
    this.container.position.set(this.anchorPosition.x, this.anchorPosition.y);
  }

  setSquash(factor: number): void {
    const breathe = 1 + Math.sin(this.breathPhase * Math.PI * 2) * 0.02;
    const base = this.baseScale * breathe * (this.carried ? 0.94 : 1);
    for (const layer of [this.body, this.eyeOpen, this.eyeClosed, this.accent]) {
      if (!layer) continue;
      const sx = this.flipped ? -(base / factor) : base / factor;
      layer.scale.set(sx, base * factor);
    }
    if (this.petMesh) {
      const sx = this.flipped ? -(base / factor) : base / factor;
      this.petMesh.mesh.scale.set(sx, base * factor);
    }
    this.applyMesh();
  }

  setShift(dx: number, dy: number): void {
    this.container.position.set(this.anchorPosition.x + dx, this.anchorPosition.y + dy);
  }

  setTilt(angleDeg: number): void {
    this.container.rotation = (angleDeg * Math.PI) / 180;
  }

  setCarried(carried: boolean): void {
    this.carried = carried;
  }

  setAccentVisible(visible: boolean): void {
    if (this.accent) this.accent.visible = visible;
  }

  private applyMesh(): void {
    if (!this.petMesh) return;
    const phase = this.breathPhase * Math.PI * 2;
    this.petMesh.setParams({
      blink: this.eyesOpen ? 0 : 1,
      earWobble: Math.sin(phase * 1.3) * 0.5,
      tailSway: phase * 0.8,
      headTurn: this.headTurn,
    });
    this.petMesh.update();
  }

  private featuresForMesh(): FeatureRects {
    const subject = this.subject
      ?? { x: 0, y: 0, width: this.source.width, height: this.source.height };
    if (this.meshFeatures) {
      const toRect = (box: ManifestFeatureBox): Rect => ({
        x: box.x * this.source.width,
        y: box.y * this.source.height,
        width: box.width * this.source.width,
        height: box.height * this.source.height,
      });
      return {
        leftEye: toRect(this.meshFeatures.leftEye),
        rightEye: toRect(this.meshFeatures.rightEye),
        leftEar: toRect(this.meshFeatures.leftEar),
        rightEar: toRect(this.meshFeatures.rightEar),
        tail: toRect(this.meshFeatures.tail),
      };
    }
    return heuristicFeatures(subject, this.source.width, this.source.height);
  }

  private drawLids(): void {
    this.lids.clear();
    if (!this.isSingleImage) return;
    const features = this.featuresForMesh();
    const arc = (eye: Rect): void => {
      const centerX = eye.x + eye.width / 2;
      const half = eye.width * 0.42;
      const dip = eye.height * 0.7;
      this.lids
        .moveTo(centerX - half, eye.y)
        .lineTo(centerX - half * 0.5, eye.y + dip)
        .lineTo(centerX, eye.y)
        .lineTo(centerX + half * 0.5, eye.y + dip)
        .lineTo(centerX + half, eye.y)
        .stroke({ width: Math.max(2, eye.height * 0.28), color: 0x5b3a29, alpha: 0.95 });
    };
    arc(features.leftEye);
    arc(features.rightEye);
    this.lids.visible = !this.eyesOpen;
  }

  /** Current visual bounds of the body layer in viewport coordinates. */
  getBodyBounds(): BodyBounds {
    const body = this.body;
    const cs = this.container.scale;
    const pos = this.container.position;
    // sprite.width already includes body.scale in Pixi; use the texture's raw
    // size so the scale is applied exactly once
    const width = body.texture.width * Math.abs(body.scale.x) * Math.abs(cs.x);
    const height = body.texture.height * Math.abs(body.scale.y) * Math.abs(cs.y);
    // body.x is the layer center (anchor 0.5, 0); the container transform is
    // applied as `position + local * scale`.
    const centerX = pos.x + body.x * cs.x;
    const topY = pos.y + body.y * cs.y;
    return {
      x: centerX - width / 2,
      y: topY,
      width,
      height,
    };
  }
}
