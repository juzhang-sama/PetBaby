export interface ProbeSample {
  webgl: boolean;
  nonTransparentPixels: number;
  contextLost: boolean;
}

export type ProbeResult =
  | { ok: true }
  | { ok: false; reason: "webgl-unavailable" | "context-lost" | "blank-frame" | "adapter-error"; message?: string };

export function isLive2DProbeMode(search: string): boolean {
  return new URLSearchParams(search).get("live2dProbe") === "1";
}

export function evaluateProbe(input: ProbeSample): ProbeResult {
  if (!input.webgl) return { ok: false, reason: "webgl-unavailable" };
  if (input.contextLost) return { ok: false, reason: "context-lost" };
  if (input.nonTransparentPixels <= 0) return { ok: false, reason: "blank-frame" };
  return { ok: true };
}

function countNonTransparentPixels(gl: WebGLRenderingContext | WebGL2RenderingContext): number {
  const pixels = new Uint8Array(gl.drawingBufferWidth * gl.drawingBufferHeight * 4);
  gl.readPixels(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
  let count = 0;
  for (let i = 3; i < pixels.length; i += 4) if (pixels[i] !== 0) count += 1;
  return count;
}

export async function mountLive2DProbe(root: HTMLElement, options: { adapter?: import("./cubism-adapter").CubismAdapter; canvas?: HTMLCanvasElement } = {}): Promise<ProbeResult> {
  const canvas = options.canvas ?? document.createElement("canvas");
  canvas.width = 256;
  canvas.height = 256;
  canvas.dataset.live2dProbe = "1";
  root.replaceChildren(canvas);
  const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: false }) ??
    canvas.getContext("webgl", { alpha: true, premultipliedAlpha: false });
  if (!gl) return evaluateProbe({ webgl: false, nonTransparentPixels: 0, contextLost: false });
  const contextLost = () => gl.isContextLost();
  const adapter = options.adapter ?? new (await import("./cubism-adapter")).UnavailableCubismAdapter();
  try {
    await adapter.initialize(canvas);
    await adapter.loadModel("/live2d/Wanko/Wanko.model3.json");
    const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
    adapter.resize(canvas.width, canvas.height, dpr);
    adapter.update(16);
    adapter.draw();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    adapter.destroy();
    return { ok: false, reason: "adapter-error", message };
  }
  canvas.addEventListener("webglcontextlost", () => undefined, { once: true });
  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.clearColor(0, 0, 0, 0);
  gl.clear(gl.COLOR_BUFFER_BIT);
  const result = evaluateProbe({ webgl: true, nonTransparentPixels: countNonTransparentPixels(gl), contextLost: contextLost() });
  if (!result.ok) adapter.destroy();
  return result;
}
