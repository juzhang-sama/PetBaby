import { createConfiguredCubismAdapter } from "./cubism-adapter";
import type { CubismAdapter } from "./cubism-adapter";
import { applyHitRegion as invokeApplyHitRegion } from "../runtime/bridge";
import type { HitRegionPayload } from "../runtime/contracts";
import { alphaToRegionSpans } from "../runtime/hit-mask";

export interface ProbeSample {
  webgl: boolean;
  nonTransparentPixels: number;
  contextLost: boolean;
}

export type ProbeResult =
  | { ok: true }
  | { ok: false; reason: "webgl-unavailable" | "context-lost" | "blank-frame" | "adapter-error" | "hit-region-error"; message?: string };

export function isLive2DProbeMode(search: string): boolean {
  return new URLSearchParams(search).get("live2dProbe") === "1";
}

export function evaluateProbe(input: ProbeSample): ProbeResult {
  if (!input.webgl) return { ok: false, reason: "webgl-unavailable" };
  if (input.contextLost) return { ok: false, reason: "context-lost" };
  if (input.nonTransparentPixels <= 0) return { ok: false, reason: "blank-frame" };
  return { ok: true };
}

function readPixels(gl: WebGLRenderingContext | WebGL2RenderingContext): Uint8Array {
  const pixels = new Uint8Array(gl.drawingBufferWidth * gl.drawingBufferHeight * 4);
  gl.readPixels(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
  return pixels;
}

function countNonTransparentPixels(pixels: Uint8Array): number {
  let count = 0;
  for (let i = 3; i < pixels.length; i += 4) if (pixels[i] !== 0) count += 1;
  return count;
}

function mapAlphaToCssRgba(
  pixels: Uint8Array,
  physicalWidth: number,
  physicalHeight: number,
  cssWidth: number,
  cssHeight: number,
): Uint8ClampedArray {
  const cssPixels = new Uint8ClampedArray(cssWidth * cssHeight * 4);
  for (let cssY = 0; cssY < cssHeight; cssY += 1) {
    const physicalTop = Math.floor(((cssHeight - cssY - 1) * physicalHeight) / cssHeight);
    const physicalBottom = Math.max(
      physicalTop + 1,
      Math.ceil(((cssHeight - cssY) * physicalHeight) / cssHeight),
    );
    for (let cssX = 0; cssX < cssWidth; cssX += 1) {
      const physicalLeft = Math.floor((cssX * physicalWidth) / cssWidth);
      const physicalRight = Math.max(
        physicalLeft + 1,
        Math.ceil(((cssX + 1) * physicalWidth) / cssWidth),
      );
      let alpha = 0;
      for (let physicalY = physicalTop; physicalY < physicalBottom; physicalY += 1) {
        for (let physicalX = physicalLeft; physicalX < physicalRight; physicalX += 1) {
          alpha = Math.max(alpha, pixels[(physicalY * physicalWidth + physicalX) * 4 + 3] ?? 0);
        }
      }
      cssPixels[(cssY * cssWidth + cssX) * 4 + 3] = alpha;
    }
  }
  return cssPixels;
}

function hasTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface ProbeOptions {
  adapter?: CubismAdapter;
  canvas?: HTMLCanvasElement;
  createAdapter?: () => Promise<CubismAdapter>;
  requestFrame?: (callback: FrameRequestCallback) => number;
  cancelFrame?: (handle: number) => void;
  devicePixelRatio?: number;
  applyHitRegion?: (payload: HitRegionPayload) => Promise<unknown>;
}

export async function mountLive2DProbe(root: HTMLElement, options: ProbeOptions = {}): Promise<ProbeResult> {
  const canvas = options.canvas ?? document.createElement("canvas");
  const bounds = typeof root.getBoundingClientRect === "function" ? root.getBoundingClientRect() : undefined;
  const width = Math.max(1, Math.round(root.clientWidth || bounds?.width || 256));
  const height = Math.max(1, Math.round(root.clientHeight || bounds?.height || 256));
  const dpr = options.devicePixelRatio ?? (typeof window === "undefined" ? 1 : window.devicePixelRatio || 1);
  canvas.width = Math.max(1, Math.round(width * dpr));
  canvas.height = Math.max(1, Math.round(height * dpr));
  canvas.style.width = "100%";
  canvas.style.height = "100%";
  canvas.style.display = "block";
  canvas.dataset.live2dProbe = "1";
  root.replaceChildren(canvas);
  const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: false }) ??
    canvas.getContext("webgl", { alpha: true, premultipliedAlpha: false });
  if (!gl) return evaluateProbe({ webgl: false, nonTransparentPixels: 0, contextLost: false });
  const contextLost = () => gl.isContextLost();
  let adapter: CubismAdapter | undefined = options.adapter;
  try {
    adapter ??= await (options.createAdapter ?? createConfiguredCubismAdapter)();
    await adapter.initialize(canvas);
    await adapter.loadModel("/live2d/Wanko/Wanko.model3.json");
    adapter.resize(width, height, dpr);
    gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    adapter.update(16);
    adapter.draw();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error("Live2D probe failed", error);
    adapter?.destroy();
    return { ok: false, reason: "adapter-error", message };
  }
  const pixels = readPixels(gl);
  const result = evaluateProbe({ webgl: true, nonTransparentPixels: countNonTransparentPixels(pixels), contextLost: contextLost() });
  if (!result.ok) {
    adapter.destroy();
    return result;
  }

  const applyHitRegion = options.applyHitRegion ?? (hasTauriRuntime() ? invokeApplyHitRegion : undefined);
  if (applyHitRegion) {
    try {
      await applyHitRegion({
        canvasWidth: width,
        canvasHeight: height,
        scaleFactor: dpr,
        spans: alphaToRegionSpans(
          mapAlphaToCssRgba(pixels, gl.drawingBufferWidth, gl.drawingBufferHeight, width, height),
          width,
          height,
          { alphaThreshold: 32, rowStep: 2 },
        ),
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      console.error("Live2D probe hit region failed", error);
      adapter.destroy();
      return { ok: false, reason: "hit-region-error", message };
    }
  }

  const browserWindow = typeof window === "undefined" ? undefined : window;
  const requestFrame = options.requestFrame
    ?? browserWindow?.requestAnimationFrame.bind(browserWindow)
    ?? (() => 0);
  const cancelFrame = options.cancelFrame
    ?? browserWindow?.cancelAnimationFrame.bind(browserWindow)
    ?? (() => undefined);
  const now = (): number => globalThis.performance?.now() ?? Date.now();
  let previousTime = now();
  let frameHandle: number | undefined;
  let stopped = false;
  const stop = (): void => {
    if (stopped) return;
    stopped = true;
    if (frameHandle !== undefined) cancelFrame(frameHandle);
    adapter?.destroy();
  };
  const renderFrame = (time: number): void => {
    if (stopped || contextLost()) return stop();
    adapter.update(Math.min(100, Math.max(0, time - previousTime)));
    adapter.draw();
    previousTime = time;
    frameHandle = requestFrame(renderFrame);
  };
  frameHandle = requestFrame(renderFrame);
  canvas.addEventListener("webglcontextlost", (event) => {
    event.preventDefault();
    stop();
  }, { once: true });
  return result;
}
