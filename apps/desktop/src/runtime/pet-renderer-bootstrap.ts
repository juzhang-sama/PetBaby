import { loadLive2DAsset } from "../runtime-assets/live2d-asset-loader";
import type { RuntimeAssetManifestV2 } from "../runtime-assets/live2d-manifest";
import { Live2DRenderer } from "../runtime-live2d/live2d-renderer";
import { loadAnimatedImageAsset } from "./animated-image-asset-loader";
import { AnimatedImageRenderer } from "./animated-image-renderer";
import { parseRuntimeAssetManifest, type RuntimeAssetManifestV1 } from "./manifest-schema";
import { installedPetAssetUrl } from "./pet-asset-url";
import type { PetRenderer } from "./pet-renderer";
import { PetRendererHost } from "./pet-renderer-host";
import { StaticPngRenderer } from "./static-png-renderer";

export type RendererKind = "live2d" | "animated-image" | "static-png";

type AnimatedPetRenderer = Omit<PetRenderer, "getHitSurface"> & {
  getHitSurface(): HTMLCanvasElement;
};

export interface RendererDiagnostic {
  petId: string;
  manifestVersion: number;
  stage:
    | "mount"
    | "manifest-load"
    | "live2d-initial-load"
    | "live2d-context-restore"
    | "static-fallback"
    | "hit-region"
    | "window-motion"
    | "fullscreen";
  message: string;
}

export interface PetRendererRuntime {
  host: PetRendererHost;
  getSurface(): HTMLCanvasElement;
  getHitSurface(): HTMLCanvasElement;
  kind(): RendererKind;
  recoverToPreview(error: unknown): Promise<void>;
}

export interface PetRendererBootstrapOptions {
  root: HTMLElement;
  createCanvas?: () => HTMLCanvasElement;
  createStaticRenderer?: (root: HTMLElement, canvas: HTMLCanvasElement) => PetRenderer;
  createLive2DRenderer?: (
    canvas: HTMLCanvasElement,
    onReloadFailure: (error: unknown) => void,
  ) => PetRenderer;
  createAnimatedRenderer?: (
    root: HTMLElement,
    displaySurface: HTMLCanvasElement,
    hitSurface: HTMLCanvasElement,
    composeSurface: HTMLCanvasElement,
  ) => AnimatedPetRenderer;
  loadLive2DAsset?: typeof loadLive2DAsset;
  loadAnimatedImageAsset?: typeof loadAnimatedImageAsset;
  assetUrl?: (petId: string, relativePath: string) => string;
  diagnose?: (diagnostic: RendererDiagnostic) => void;
  onSurfaceChanged?: () => void | Promise<void>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function preferredV1Image(manifest: RuntimeAssetManifestV1): string {
  return manifest.files.find((file) => file.role === "main" || file.role === "body")?.relativePath
    ?? manifest.files[0]!.relativePath;
}

export async function createStaticPngRuntime(
  imageUrl: string,
  options: PetRendererBootstrapOptions,
): Promise<PetRendererRuntime> {
  const createCanvas = options.createCanvas ?? (() => document.createElement("canvas"));
  const createStaticRenderer = options.createStaticRenderer
    ?? ((root, canvas) => new StaticPngRenderer(root, { createCanvas: () => canvas }));
  const surface = createCanvas();
  surface.className = "pet-render-surface";
  const renderer = createStaticRenderer(options.root, surface);
  const host = new PetRendererHost(renderer);
  try {
    await host.load({ kind: "static-png", imageUrl });
  } catch (error) {
    host.destroy();
    throw error;
  }
  return {
    host,
    getSurface: () => surface,
    getHitSurface: () => surface,
    kind: () => "static-png",
    recoverToPreview: async () => undefined,
  };
}

export async function createPetRendererRuntime(
  petId: string,
  manifestJson: unknown,
  options: PetRendererBootstrapOptions,
): Promise<PetRendererRuntime> {
  const manifest = parseRuntimeAssetManifest(manifestJson);
  if (manifest.petId !== petId) throw new Error("manifest petId does not match the active pet");

  const createCanvas = options.createCanvas ?? (() => document.createElement("canvas"));
  const createStaticRenderer = options.createStaticRenderer
    ?? ((root, canvas) => new StaticPngRenderer(root, { createCanvas: () => canvas }));
  const createLive2DRenderer = options.createLive2DRenderer
    ?? ((canvas, onReloadFailure) => new Live2DRenderer(canvas, { onReloadFailure }));
  const createAnimatedRenderer = options.createAnimatedRenderer
    ?? ((root, displaySurface, hitSurface, composeSurface) => {
      const surfaces = [displaySurface, hitSurface, composeSurface];
      let surfaceIndex = 0;
      return new AnimatedImageRenderer(root, {
        createCanvas: () => surfaces[surfaceIndex++]!,
      });
    });
  const assetUrl = options.assetUrl ?? installedPetAssetUrl;

  const makeStatic = (relativePath: string): Promise<PetRendererRuntime> => createStaticPngRuntime(
    assetUrl(petId, relativePath),
    options,
  );

  if (manifest.schemaVersion === 1) return makeStatic(preferredV1Image(manifest));
  if (manifest.schemaVersion === 3) {
    const displaySurface = createCanvas();
    displaySurface.className = "pet-render-surface";
    const hitSurface = createCanvas();
    const composeSurface = createCanvas();
    const animatedRenderer = createAnimatedRenderer(
      options.root,
      displaySurface,
      hitSurface,
      composeSurface,
    );
    const host = new PetRendererHost(animatedRenderer);
    try {
      const asset = await (options.loadAnimatedImageAsset ?? loadAnimatedImageAsset)(
        petId,
        manifest,
        assetUrl,
      );
      await host.load(asset);
      options.root.replaceChildren(displaySurface);
    } catch (error) {
      host.destroy();
      throw error;
    }
    return {
      host,
      getSurface: () => displaySurface,
      getHitSurface: () => animatedRenderer.getHitSurface(),
      kind: () => "animated-image",
      recoverToPreview: async () => undefined,
    };
  }

  const liveManifest: RuntimeAssetManifestV2 = manifest;
  const previewAsset = {
    kind: "static-png" as const,
    imageUrl: assetUrl(petId, liveManifest.previewImage),
  };
  let runtime: PetRendererRuntime | null = null;
  const liveSurface = createCanvas();
  liveSurface.className = "pet-render-surface";
  const liveRenderer = createLive2DRenderer(liveSurface, (error) => {
    void runtime?.recoverToPreview(error).catch(() => undefined);
  });
  const host = new PetRendererHost(liveRenderer);

  try {
    const liveAsset = await (options.loadLive2DAsset ?? loadLive2DAsset)(petId, liveManifest);
    await host.load(liveAsset);
    options.root.replaceChildren(liveSurface);
  } catch (error) {
    host.destroy();
    options.diagnose?.({
      petId,
      manifestVersion: liveManifest.schemaVersion,
      stage: "live2d-initial-load",
      message: errorMessage(error),
    });
    return makeStatic(liveManifest.previewImage);
  }

  let surface = liveSurface;
  let rendererKind: RendererKind = "live2d";
  let recovery: Promise<void> | null = null;
  runtime = {
    host,
    getSurface: () => surface,
    getHitSurface: () => surface,
    kind: () => rendererKind,
    recoverToPreview: async (error) => {
      if (rendererKind !== "live2d") return;
      if (recovery) return recovery;
      options.diagnose?.({
        petId,
        manifestVersion: liveManifest.schemaVersion,
        stage: "live2d-context-restore",
        message: errorMessage(error),
      });
      recovery = (async () => {
        const fallbackSurface = createCanvas();
        fallbackSurface.className = "pet-render-surface";
        const fallback = createStaticRenderer(options.root, fallbackSurface);
        try {
          await host.replace(fallback, previewAsset);
          surface = fallbackSurface;
          rendererKind = "static-png";
          await options.onSurfaceChanged?.();
        } catch (fallbackError) {
          options.diagnose?.({
            petId,
            manifestVersion: liveManifest.schemaVersion,
            stage: "static-fallback",
            message: errorMessage(fallbackError),
          });
          throw fallbackError;
        }
      })();
      return recovery;
    },
  };
  return runtime;
}
