import { parseLive2DManifest } from "../runtime-assets/live2d-manifest";
import type { PetRenderer } from "../runtime/pet-renderer";
import { Live2DRenderer } from "./live2d-renderer";

const DEFAULT_MANIFEST_URL = "/builtin-pets/pet-live2d-v1/manifest.json";

export interface Live2DPreviewSession {
  destroy(): void;
}

interface Live2DPreviewOptions {
  manifestUrl?: string;
  fetchJson?: (url: string) => Promise<unknown>;
  createCanvas?: () => HTMLCanvasElement;
  createRenderer?: (canvas: HTMLCanvasElement) => PetRenderer;
  requestFrame?: (callback: FrameRequestCallback) => number;
  cancelFrame?: (handle: number) => void;
  origin?: string;
  devicePixelRatio?: number;
}

export function isLive2DPreviewMode(search: string): boolean {
  return new URLSearchParams(search).get("live2dPreview") === "1";
}

export function resolvePreviewUrl(manifestUrl: string, relative: string, origin: string): string {
  return new URL(relative, new URL(manifestUrl, origin)).toString();
}

async function fetchJson(url: string): Promise<unknown> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Live2D preview manifest failed (${response.status}): ${url}`);
  return response.json();
}

export async function mountLive2DPreview(
  root: HTMLElement,
  options: Live2DPreviewOptions = {},
): Promise<Live2DPreviewSession> {
  const manifestUrl = options.manifestUrl ?? DEFAULT_MANIFEST_URL;
  const manifest = parseLive2DManifest(await (options.fetchJson ?? fetchJson)(manifestUrl));
  const canvas = options.createCanvas?.() ?? document.createElement("canvas");
  const renderer = options.createRenderer?.(canvas) ?? new Live2DRenderer(canvas);
  const origin = options.origin ?? window.location.origin;
  const requestFrame = options.requestFrame ?? window.requestAnimationFrame.bind(window);
  const cancelFrame = options.cancelFrame ?? window.cancelAnimationFrame.bind(window);

  canvas.className = "pet-render-surface";
  canvas.dataset.live2dPreview = "1";
  root.replaceChildren(canvas);

  const resize = (): void => {
    const bounds = root.getBoundingClientRect();
    renderer.resize({
      width: Math.max(1, Math.round(root.clientWidth || bounds.width || 420)),
      height: Math.max(1, Math.round(root.clientHeight || bounds.height || 520)),
      dpr: Math.max(1, options.devicePixelRatio ?? (window.devicePixelRatio || 1)),
    });
  };

  let stopped = false;
  let frameHandle: number | undefined;
  let previousTime = performance.now();
  const destroy = (): void => {
    if (stopped) return;
    stopped = true;
    if (frameHandle !== undefined) cancelFrame(frameHandle);
    window.removeEventListener("resize", resize);
    window.removeEventListener("beforeunload", destroy);
    renderer.destroy();
  };
  const renderFrame = (time: number): void => {
    if (stopped) return;
    renderer.update(Math.min(100, Math.max(0, time - previousTime)));
    previousTime = time;
    frameHandle = requestFrame(renderFrame);
  };

  try {
    await renderer.load({
      kind: "live2d",
      modelUrl: resolvePreviewUrl(manifestUrl, manifest.modelEntry, origin),
      previewUrl: resolvePreviewUrl(manifestUrl, manifest.previewImage, origin),
      semantics: manifest.semantics,
      dispose() {},
    });
    resize();
    renderer.setVisibility(true);
    renderer.update(0);
    window.addEventListener("resize", resize);
    window.addEventListener("beforeunload", destroy, { once: true });
    frameHandle = requestFrame(renderFrame);
    root.dataset.live2dPreviewResult = "ok";
    return { destroy };
  } catch (error) {
    root.dataset.live2dPreviewResult = "error";
    destroy();
    throw error;
  }
}
