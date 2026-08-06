import { evaluateProbe, isLive2DProbeMode, mountLive2DProbe } from "./probe";
import { describe, expect, it, vi } from "vitest";

describe("Live2D probe evaluation", () => {
  it("selects the probe entry only for an explicit query flag", () => {
    expect(isLive2DProbeMode("?live2dProbe=1")).toBe(true);
    expect(isLive2DProbeMode("?live2dProbe=0")).toBe(false);
    expect(isLive2DProbeMode("")).toBe(false);
  });

  it("rejects a blank frame", () => {
    expect(
      evaluateProbe({ webgl: true, nonTransparentPixels: 0, contextLost: false }),
    ).toEqual({ ok: false, reason: "blank-frame" });
  });

  it("rejects an invalid negative pixel count", () => {
    expect(
      evaluateProbe({ webgl: true, nonTransparentPixels: -1, contextLost: false }),
    ).toEqual({ ok: false, reason: "blank-frame" });
  });

  it("accepts a rendered frame", () => {
    expect(
      evaluateProbe({ webgl: true, nonTransparentPixels: 1200, contextLost: false }),
    ).toEqual({ ok: true });
  });

  it("reports unavailable WebGL before frame state", () => {
    expect(
      evaluateProbe({ webgl: false, nonTransparentPixels: 1200, contextLost: false }),
    ).toEqual({ ok: false, reason: "webgl-unavailable" });
  });

  it("reports a lost context before frame state", () => {
    expect(
      evaluateProbe({ webgl: true, nonTransparentPixels: 1200, contextLost: true }),
    ).toEqual({ ok: false, reason: "context-lost" });
  });

  it("renders through the Cubism adapter and samples alpha", async () => {
    const update = vi.fn();
    const draw = vi.fn();
    const destroy = vi.fn();
    const adapter = { initialize: vi.fn(async () => {}), loadModel: vi.fn(async () => {}), resize: vi.fn(), update, draw, destroy };
    const gl = { drawingBufferWidth: 1, drawingBufferHeight: 1, RGBA: 1, UNSIGNED_BYTE: 2, viewport: vi.fn(), clearColor: vi.fn(), clear: vi.fn(), readPixels: vi.fn((_x: number, _y: number, _w: number, _h: number, _f: number, _t: number, pixels: Uint8Array) => { pixels[3] = 255; }), isContextLost: () => false, COLOR_BUFFER_BIT: 4 };
    const canvas = { width: 0, height: 0, dataset: {} as DOMStringMap, getContext: vi.fn(() => gl), addEventListener: vi.fn() } as unknown as HTMLCanvasElement;
    const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
    const result = await mountLive2DProbe(root, { adapter, canvas });
    expect(result).toEqual({ ok: true });
    expect(adapter.initialize).toHaveBeenCalled();
    expect(adapter.loadModel).toHaveBeenCalledWith("/live2d/Wanko/Wanko.model3.json");
    expect(update).toHaveBeenCalled();
    expect(draw).toHaveBeenCalled();
    adapter.destroy();
    expect(destroy).toHaveBeenCalled();
  });

  it("returns a diagnostic failure when Cubism initialization fails", async () => {
    const adapter = { initialize: vi.fn(async () => { throw new Error("Core missing"); }), loadModel: vi.fn(), resize: vi.fn(), update: vi.fn(), draw: vi.fn(), destroy: vi.fn() };
    const canvas = { width: 0, height: 0, dataset: {} as DOMStringMap, getContext: vi.fn(() => ({ isContextLost: () => false })), addEventListener: vi.fn() } as unknown as HTMLCanvasElement;
    const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
    const result = await mountLive2DProbe(root, { adapter, canvas });
    expect(result).toEqual({ ok: false, reason: "adapter-error", message: "Core missing" });
  });
});
