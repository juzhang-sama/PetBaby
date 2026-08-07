import { describe, expect, it, vi } from "vitest";
import type { PetRenderAsset } from "../runtime/pet-renderer";
import { Live2DRenderer } from "./live2d-renderer";
import type { LoadedCubismModel } from "./cubism-model-loader";

function liveAsset(id = "a"): Extract<PetRenderAsset, { kind: "live2d" }> {
  return {
    kind: "live2d",
    modelUrl: `blob:model-${id}`,
    previewUrl: `blob:preview-${id}`,
    semantics: {
      motions: { idle: { group: "Idle", index: 0 }, carried: { group: "Carry", index: 0 } },
      expressions: { happy: "Happy" },
      hitAreas: { head: "Head" },
      parameters: { eyeOpen: "Eye", mouthOpen: "Mouth" },
    },
    dispose: vi.fn(),
  };
}

function fakeModel(): LoadedCubismModel {
  return {
    resize: vi.fn(),
    update: vi.fn(),
    draw: vi.fn(),
    release: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    stopAllMotions: vi.fn(),
    setExpression: vi.fn(),
    setParameter: vi.fn(),
    getParameterRange: vi.fn(() => ({ min: 0, max: 1 })),
    hitTest: vi.fn(() => true),
  };
}

function fakeCanvas() {
  const listeners = new Map<string, EventListener>();
  return {
    width: 0,
    height: 0,
    style: {},
    addEventListener: vi.fn((name: string, listener: EventListener) => listeners.set(name, listener)),
    removeEventListener: vi.fn((name: string) => listeners.delete(name)),
    emit(name: string, event: Event) { listeners.get(name)?.(event); },
  } as unknown as HTMLCanvasElement & { emit(name: string, event: Event): void };
}

describe("Live2DRenderer", () => {
  it("rejects static assets", async () => {
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn() } });
    await expect(renderer.load({ kind: "static-png", imageUrl: "pet.png" })).rejects.toThrow(/live2d/i);
  });

  it("releases the old model and asset before replacing them", async () => {
    const firstModel = fakeModel();
    const secondModel = fakeModel();
    const loader = { load: vi.fn().mockResolvedValueOnce(firstModel).mockResolvedValueOnce(secondModel) };
    const first = liveAsset("first");
    const second = liveAsset("second");
    const renderer = new Live2DRenderer(fakeCanvas(), { loader });

    await renderer.load(first);
    await renderer.load(second);

    expect(firstModel.release).toHaveBeenCalledOnce();
    expect(first.dispose).toHaveBeenCalledOnce();
    expect(renderer.state().status).toBe("ready");
  });

  it("does not let an older asynchronous load replace the current model", async () => {
    let resolveFirst!: (model: LoadedCubismModel) => void;
    const firstPending = new Promise<LoadedCubismModel>((resolve) => { resolveFirst = resolve; });
    const staleModel = fakeModel();
    const currentModel = fakeModel();
    const loader = { load: vi.fn().mockReturnValueOnce(firstPending).mockResolvedValueOnce(currentModel) };
    const renderer = new Live2DRenderer(fakeCanvas(), { loader });

    const firstLoad = renderer.load(liveAsset("first"));
    await renderer.load(liveAsset("second"));
    resolveFirst(staleModel);
    await firstLoad;

    expect(staleModel.release).toHaveBeenCalledOnce();
    renderer.setVisibility(true);
    renderer.update(16);
    expect(currentModel.update).toHaveBeenCalledOnce();
  });

  it("pauses on context loss and reloads at most once after restoration", async () => {
    const canvas = fakeCanvas();
    const firstModel = fakeModel();
    const restoredModel = fakeModel();
    const loader = { load: vi.fn().mockResolvedValueOnce(firstModel).mockResolvedValueOnce(restoredModel) };
    const renderer = new Live2DRenderer(canvas, { loader });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);

    canvas.emit("webglcontextlost", { preventDefault: vi.fn() } as unknown as Event);
    renderer.update(16);
    expect(firstModel.update).not.toHaveBeenCalled();

    canvas.emit("webglcontextrestored", new Event("webglcontextrestored"));
    canvas.emit("webglcontextrestored", new Event("webglcontextrestored"));
    await vi.waitFor(() => expect(loader.load).toHaveBeenCalledTimes(2));
    expect(renderer.state().status).toBe("ready");
  });

  it("does not let a pending load become ready after context loss", async () => {
    let resolveLoad!: (model: LoadedCubismModel) => void;
    const pending = new Promise<LoadedCubismModel>((resolve) => { resolveLoad = resolve; });
    const canvas = fakeCanvas();
    const staleModel = fakeModel();
    const renderer = new Live2DRenderer(canvas, { loader: { load: vi.fn(() => pending) } });

    const load = renderer.load(liveAsset());
    canvas.emit("webglcontextlost", { preventDefault: vi.fn() } as unknown as Event);
    resolveLoad(staleModel);
    await load;

    expect(staleModel.release).toHaveBeenCalledOnce();
    expect(renderer.state().status).toBe("context-lost");
  });

  it("does not let a missing high-priority motion block a mapped motion", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());

    renderer.playMotion("landed", { priority: 80 });
    renderer.playMotion("idle", { priority: 10, loop: true });

    expect(model.playMotion).toHaveBeenCalledOnce();
    expect(model.playMotion).toHaveBeenCalledWith("Idle", 0, { priority: 10, loop: true }, expect.any(Function));
  });

  it("queues semantic parameter writes before the SDK update and draw", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);
    renderer.setLipSync(0.6);

    renderer.update(16);

    expect(model.setParameter).toHaveBeenCalledWith("Mouth", 0.6);
    expect(vi.mocked(model.setParameter).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(model.update).mock.invocationCallOrder[0]!);
    expect(vi.mocked(model.update).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(model.draw).mock.invocationCallOrder[0]!);
  });

  it("does not force the mouth closed before lip sync is enabled", async () => {
    const model = fakeModel();
    const renderer = new Live2DRenderer(fakeCanvas(), { loader: { load: vi.fn(async () => model) } });
    await renderer.load(liveAsset());
    renderer.setVisibility(true);

    renderer.update(16);

    expect(model.setParameter).not.toHaveBeenCalledWith("Mouth", expect.any(Number));
  });

  it("destroys model, asset and listeners idempotently", async () => {
    const canvas = fakeCanvas();
    const model = fakeModel();
    const asset = liveAsset();
    const renderer = new Live2DRenderer(canvas, { loader: { load: vi.fn(async () => model) } });
    await renderer.load(asset);

    renderer.destroy();
    renderer.destroy();

    expect(model.release).toHaveBeenCalledOnce();
    expect(asset.dispose).toHaveBeenCalledOnce();
    expect(canvas.removeEventListener).toHaveBeenCalledTimes(2);
    expect(renderer.state()).toMatchObject({ status: "destroyed", visible: false });
  });
});
