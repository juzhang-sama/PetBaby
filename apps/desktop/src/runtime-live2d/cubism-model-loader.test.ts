import { describe, expect, it, vi } from "vitest";
import { CubismModelLoader, resolveCubismResourceUrl, type CubismControlAdapter } from "./cubism-model-loader";

function fakeAdapter(): CubismControlAdapter {
  return {
    initialize: vi.fn(async () => undefined),
    loadModel: vi.fn(async () => undefined),
    resize: vi.fn(),
    update: vi.fn(),
    draw: vi.fn(),
    destroy: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    stopAllMotions: vi.fn(),
    setExpression: vi.fn(),
    setParameter: vi.fn(),
    getParameterRange: vi.fn(() => ({ min: -1, max: 1 })),
    hitTest: vi.fn(() => false),
  };
}

describe("CubismModelLoader", () => {
  it("keeps absolute object URLs instead of prefixing the model directory", () => {
    expect(resolveCubismResourceUrl("blob:http://localhost/model", "blob:http://localhost/moc"))
      .toBe("blob:http://localhost/moc");
  });

  it("resolves ordinary relative resources from the model URL", () => {
    expect(resolveCubismResourceUrl("/live2d/Wanko/Wanko.model3.json", "Wanko.moc3"))
      .toBe("/live2d/Wanko/Wanko.moc3");
  });

  it("destroys a partially initialized adapter when loading fails", async () => {
    const adapter = fakeAdapter();
    vi.mocked(adapter.loadModel).mockRejectedValueOnce(new Error("bad model"));
    const loader = new CubismModelLoader(async () => adapter);

    await expect(loader.load({} as HTMLCanvasElement, "blob:model")).rejects.toThrow("bad model");
    expect(adapter.destroy).toHaveBeenCalledOnce();
  });

  it("rejects adapters that only implement the probe rendering surface", async () => {
    const destroy = vi.fn();
    const probeOnly = {
      initialize: vi.fn(async () => undefined),
      loadModel: vi.fn(async () => undefined),
      resize: vi.fn(),
      update: vi.fn(),
      draw: vi.fn(),
      destroy,
    } as unknown as CubismControlAdapter;
    const loader = new CubismModelLoader(async () => probeOnly);

    await expect(loader.load({} as HTMLCanvasElement, "blob:model")).rejects.toThrow(/playMotion/);
    expect(destroy).toHaveBeenCalledOnce();
  });
});
