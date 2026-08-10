import type { ComposerRecipe } from "./contracts";
import type { ComposerPackManifest, ComposerPart } from "./composer-pack";
import { validateRecipe } from "./composer-pack";
import {
  parseMotionProfile,
  type MotionProfileV1,
} from "../runtime/animated-image-manifest";

const CANVAS_SIZE = 1024;

export type ComposerLayerKind = "tail" | "body" | "ears" | "eyes-open" | "muzzle";

export interface ComposerRenderLayer {
  kind: ComposerLayerKind;
  id: string;
  image: string;
  colorMask?: string;
  patternMask?: string;
  zIndex: number;
}

export interface ComposerRenderPorts {
  createSurface(width: number, height: number): HTMLCanvasElement;
  context(surface: HTMLCanvasElement): CanvasRenderingContext2D;
  loadImage(url: string): Promise<CanvasImageSource>;
  assetUrl(relativePath: string): string;
  toPng(surface: HTMLCanvasElement): Promise<Blob>;
}

const SEMANTIC_ORDER: Record<ComposerLayerKind, number> = {
  tail: 0,
  body: 1,
  ears: 2,
  "eyes-open": 3,
  muzzle: 4,
};

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function assertValidRecipe(pack: ComposerPackManifest, recipe: ComposerRecipe): void {
  const errors = validateRecipe(pack, recipe);
  if (errors.length > 0) throw new Error(`invalid composer recipe: ${errors.join("; ")}`);
}

function layerFromPart(kind: ComposerLayerKind, part: ComposerPart): ComposerRenderLayer {
  return {
    kind,
    id: part.id,
    image: part.image,
    ...(part.colorMask === undefined ? {} : { colorMask: part.colorMask }),
    ...(part.patternMask === undefined ? {} : { patternMask: part.patternMask }),
    zIndex: part.zIndex,
  };
}

export function planComposerRecipe(
  pack: ComposerPackManifest,
  recipe: ComposerRecipe,
): ComposerRenderLayer[] {
  assertValidRecipe(pack, recipe);
  const body = pack.bodies.find((item) => item.id === recipe.bodyId)!;
  const ears = pack.ears.find((item) => item.id === recipe.earsId)!;
  const eyes = pack.eyes.find((item) => item.id === recipe.eyesId)!;
  const muzzle = pack.muzzles.find((item) => item.id === recipe.muzzleId)!;
  const tail = pack.tails.find((item) => item.id === recipe.tailId)!;
  const layers: ComposerRenderLayer[] = [
    layerFromPart("tail", tail),
    layerFromPart("body", body),
    layerFromPart("ears", ears),
    {
      kind: "eyes-open",
      id: eyes.id,
      image: eyes.openImage,
      ...(eyes.colorMask === undefined ? {} : { colorMask: eyes.colorMask }),
      ...(eyes.patternMask === undefined ? {} : { patternMask: eyes.patternMask }),
      zIndex: eyes.zIndex,
    },
    layerFromPart("muzzle", muzzle),
  ];
  return layers.sort((left, right) =>
    left.zIndex - right.zIndex
    || SEMANTIC_ORDER[left.kind] - SEMANTIC_ORDER[right.kind]
    || compareText(left.id, right.id)
    || compareText(left.image, right.image),
  );
}

function newSurface(ports: ComposerRenderPorts): HTMLCanvasElement {
  const surface = ports.createSurface(CANVAS_SIZE, CANVAS_SIZE);
  if (surface.width !== CANVAS_SIZE || surface.height !== CANVAS_SIZE) {
    throw new Error("composer surface must be 1024x1024");
  }
  return surface;
}

function drawSafely(
  context: CanvasRenderingContext2D,
  draw: () => void,
): void {
  context.save();
  try {
    context.globalAlpha = 1;
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.globalCompositeOperation = "source-over";
    draw();
  } finally {
    context.restore();
  }
}

function requireImage(images: ReadonlyMap<string, CanvasImageSource>, path: string): CanvasImageSource {
  const image = images.get(path);
  if (!image) throw new Error(`composer image was not loaded: ${path}`);
  return image;
}

function renderColorOverlay(
  ports: ComposerRenderPorts,
  mask: CanvasImageSource,
  base: CanvasImageSource,
  color: string,
): HTMLCanvasElement {
  const surface = newSurface(ports);
  const context = ports.context(surface);
  drawSafely(context, () => {
    context.drawImage(mask, 0, 0);
    context.globalCompositeOperation = "source-in";
    context.fillStyle = color;
    context.fillRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);
    context.globalCompositeOperation = "multiply";
    context.drawImage(base, 0, 0);
  });
  return surface;
}

function renderPatternOverlay(
  ports: ComposerRenderPorts,
  pattern: CanvasImageSource,
  mask: CanvasImageSource,
  base: CanvasImageSource,
): HTMLCanvasElement {
  const surface = newSurface(ports);
  const context = ports.context(surface);
  drawSafely(context, () => {
    context.drawImage(pattern, 0, 0);
    context.globalCompositeOperation = "destination-in";
    context.drawImage(mask, 0, 0);
    context.globalCompositeOperation = "destination-in";
    context.drawImage(base, 0, 0);
  });
  return surface;
}

function renderLayer(
  ports: ComposerRenderPorts,
  layer: ComposerRenderLayer,
  images: ReadonlyMap<string, CanvasImageSource>,
  color: string,
  patternImage: string | null,
): HTMLCanvasElement {
  const surface = newSurface(ports);
  const context = ports.context(surface);
  const base = requireImage(images, layer.image);
  drawSafely(context, () => {
    context.drawImage(base, 0, 0);
    if (layer.colorMask) {
      const overlay = renderColorOverlay(
        ports,
        requireImage(images, layer.colorMask),
        base,
        color,
      );
      context.globalCompositeOperation = "source-atop";
      context.drawImage(overlay, 0, 0);
    }
    if (patternImage && layer.patternMask) {
      const overlay = renderPatternOverlay(
        ports,
        requireImage(images, patternImage),
        requireImage(images, layer.patternMask),
        base,
      );
      context.globalCompositeOperation = "source-over";
      context.drawImage(overlay, 0, 0);
    }
  });
  return surface;
}

function assetPaths(layers: readonly ComposerRenderLayer[], patternImage: string | null): string[] {
  const paths: string[] = [];
  const seen = new Set<string>();
  const add = (path: string | undefined) => {
    if (path !== undefined && !seen.has(path)) {
      seen.add(path);
      paths.push(path);
    }
  };
  for (const layer of layers) {
    add(layer.image);
    add(layer.colorMask);
    if (patternImage && layer.patternMask) {
      add(patternImage);
      add(layer.patternMask);
    }
  }
  return paths;
}

async function loadAssets(
  ports: ComposerRenderPorts,
  paths: readonly string[],
): Promise<Map<string, CanvasImageSource>> {
  const images = new Map<string, CanvasImageSource>();
  for (const path of paths) {
    images.set(path, await ports.loadImage(ports.assetUrl(path)));
  }
  return images;
}

async function renderOffscreen(
  pack: ComposerPackManifest,
  recipe: ComposerRecipe,
  ports: ComposerRenderPorts,
  layers: readonly ComposerRenderLayer[],
): Promise<HTMLCanvasElement> {
  const color = pack.colors.find((item) => item.id === recipe.colorId)!;
  const pattern = pack.patterns.find((item) => item.id === recipe.patternId)!;
  const images = await loadAssets(ports, assetPaths(layers, pattern.image));
  const renderedLayers = layers.map((layer) =>
    renderLayer(ports, layer, images, color.value, pattern.image),
  );
  const final = newSurface(ports);
  const context = ports.context(final);
  drawSafely(context, () => {
    context.clearRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);
    context.globalCompositeOperation = "source-over";
    for (const layer of renderedLayers) context.drawImage(layer, 0, 0);
  });
  return final;
}

export async function renderComposerRecipe(
  pack: ComposerPackManifest,
  recipe: ComposerRecipe,
  target: HTMLCanvasElement,
  ports: ComposerRenderPorts,
): Promise<void> {
  const layers = planComposerRecipe(pack, recipe);
  if (target.width !== CANVAS_SIZE || target.height !== CANVAS_SIZE) {
    throw new Error("composer target must be 1024x1024");
  }
  const final = await renderOffscreen(pack, recipe, ports, layers);
  const context = ports.context(target);
  drawSafely(context, () => {
    context.clearRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);
    context.globalCompositeOperation = "source-over";
    context.drawImage(final, 0, 0);
  });
}

export async function exportComposerPng(
  pack: ComposerPackManifest,
  recipe: ComposerRecipe,
  ports: ComposerRenderPorts,
): Promise<Blob> {
  const layers = planComposerRecipe(pack, recipe);
  const surface = await renderOffscreen(pack, recipe, ports, layers);
  const blob = await ports.toPng(surface);
  if (blob.type !== "image/png" || blob.size === 0) {
    throw new Error("组合 PNG 导出失败");
  }
  return blob;
}

function positiveAreaOverlap(
  first: { left: number; top: number; right: number; bottom: number },
  second: { left: number; top: number; right: number; bottom: number },
): boolean {
  return Math.max(first.left, second.left) < Math.min(first.right, second.right)
    && Math.max(first.top, second.top) < Math.min(first.bottom, second.bottom);
}

export function motionProfileForRecipe(
  pack: ComposerPackManifest,
  recipe: ComposerRecipe,
): MotionProfileV1 {
  assertValidRecipe(pack, recipe);
  const body = pack.bodies.find((item) => item.id === recipe.bodyId)!;
  if (positiveAreaOverlap(body.faceSafeZone, body.breathZone)) {
    throw new Error("composer breath zone overlaps the face safe zone");
  }
  const normalizeRect = (rect: { left: number; top: number; right: number; bottom: number }) => ({
    left: rect.left / CANVAS_SIZE,
    top: rect.top / CANVAS_SIZE,
    right: rect.right / CANVAS_SIZE,
    bottom: rect.bottom / CANVAS_SIZE,
  });
  return parseMotionProfile({
    profileVersion: 1,
    engineProfile: "life-v1",
    alphaBounds: normalizeRect(body.alphaBounds),
    breathZone: normalizeRect(body.breathZone),
    swayPivot: {
      x: body.swayPivot.x / CANVAS_SIZE,
      y: body.swayPivot.y / CANVAS_SIZE,
    },
  });
}
