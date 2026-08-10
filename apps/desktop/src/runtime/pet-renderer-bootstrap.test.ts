import { describe, expect, it, vi } from "vitest";
import type { RuntimeAssetManifestV2 } from "../runtime-assets/live2d-manifest";
import { validAnimatedManifest, validMotionProfile } from "./animated-image-test-fixtures";
import type { PetRenderAsset, PetRenderer } from "./pet-renderer";
import { createPetRendererRuntime, type RendererDiagnostic } from "./pet-renderer-bootstrap";

function fakeRenderer(loadError?: Error): PetRenderer {
  return {
    load: vi.fn(async () => { if (loadError) throw loadError; }),
    resize: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    setExpression: vi.fn(),
    setLookTarget: vi.fn(),
    setLipSync: vi.fn(),
    hitTest: vi.fn(() => null),
    setVisibility: vi.fn(),
    update: vi.fn(),
    destroy: vi.fn(),
  };
}

const v1Manifest = {
  schemaVersion: 1,
  assetType: "single-image",
  petId: "pet-a",
  variantId: "v1",
  styleId: "signature-cartoon-v1",
  view: "front",
  pose: "sitting",
  files: [
    { role: "thumbnail", relativePath: "thumb.png", sha256: "ab".repeat(32) },
    { role: "main", relativePath: "portrait.png", sha256: "cd".repeat(32) },
  ],
  animation: { idleFps: 12, blinkMsMin: 3_000, blinkMsMax: 8_000 },
};

const v2Manifest: RuntimeAssetManifestV2 = {
  schemaVersion: 2,
  renderer: "live2d-v1",
  petId: "pet-a",
  variantId: "v2",
  modelEntry: "model.model3.json",
  previewImage: "preview.png",
  files: [
    { role: "model", relativePath: "model.model3.json", sha256: "ab".repeat(32) },
    { role: "preview", relativePath: "preview.png", sha256: "cd".repeat(32) },
  ],
  semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
  license: { id: "test", author: "test", source: "test", commercialUse: true, redistributable: false },
};

function harness(options: { liveLoadError?: Error } = {}) {
  const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
  const canvases = [
    { className: "", style: {} },
    { className: "", style: {} },
    { className: "", style: {} },
  ] as unknown as HTMLCanvasElement[];
  const animatedHitSurface = canvases[1]!;
  const staticRenderers: PetRenderer[] = [];
  const liveRenderer = fakeRenderer(options.liveLoadError);
  const animatedRenderer = Object.assign(fakeRenderer(), {
    getHitSurface: vi.fn(() => animatedHitSurface),
  });
  const animatedAsset: Extract<PetRenderAsset, { kind: "animated-image" }> = {
    kind: "animated-image",
    imageUrl: "asset://pet-user-1/body.png",
    motionProfile: validMotionProfile(),
  };
  const createAnimatedRenderer = vi.fn(() => animatedRenderer);
  let reloadFailure: ((error: unknown) => void) | undefined;
  const liveAsset: Extract<PetRenderAsset, { kind: "live2d" }> = {
    kind: "live2d",
    modelUrl: "blob:model",
    previewUrl: "blob:preview",
    semantics: v2Manifest.semantics,
    dispose: vi.fn(),
  };
  const diagnostics: RendererDiagnostic[] = [];
  return {
    diagnostics,
    animatedRenderer,
    createAnimatedRenderer,
    liveRenderer,
    reload: (error: unknown) => reloadFailure?.(error),
    root,
    staticRenderers,
    options: {
      root,
      createCanvas: vi.fn(() => canvases.shift()!),
      createStaticRenderer: vi.fn((_root: HTMLElement, _canvas: HTMLCanvasElement) => {
        const renderer = fakeRenderer();
        staticRenderers.push(renderer);
        return renderer;
      }),
      createLive2DRenderer: vi.fn((_canvas: HTMLCanvasElement, onReloadFailure: (error: unknown) => void) => {
        reloadFailure = onReloadFailure;
        return liveRenderer;
      }),
      createAnimatedRenderer,
      loadLive2DAsset: vi.fn(async () => liveAsset),
      loadAnimatedImageAsset: vi.fn(async () => animatedAsset),
      assetUrl: (petId: string, path: string) => `asset://${petId}/${path}`,
      diagnose: (diagnostic: RendererDiagnostic) => diagnostics.push(diagnostic),
    },
  };
}

describe("createPetRendererRuntime", () => {
  it("routes a v1 manifest to its preferred static PNG", async () => {
    const test = harness();

    const runtime = await createPetRendererRuntime("pet-a", v1Manifest, test.options);

    expect(test.options.createLive2DRenderer).not.toHaveBeenCalled();
    expect(test.staticRenderers[0]?.load).toHaveBeenCalledWith({
      kind: "static-png",
      imageUrl: "asset://pet-a/portrait.png",
    });
    runtime.host.update(16);
    expect(test.staticRenderers[0]?.update).toHaveBeenCalledWith(16);
  });

  it("routes a v2 manifest to Live2D and mounts its surface", async () => {
    const test = harness();

    const runtime = await createPetRendererRuntime("pet-a", v2Manifest, test.options);

    expect(test.options.loadLive2DAsset).toHaveBeenCalledWith("pet-a", v2Manifest);
    expect(test.liveRenderer.load).toHaveBeenCalledWith(expect.objectContaining({ kind: "live2d" }));
    expect(test.root.replaceChildren).toHaveBeenCalledWith(runtime.getSurface());
    expect(runtime.kind()).toBe("live2d");
  });

  it("boots schema v3 through the animated image renderer", async () => {
    const test = harness();

    const runtime = await createPetRendererRuntime(
      "pet-user-1",
      validAnimatedManifest(),
      test.options,
    );

    expect(runtime.kind()).toBe("animated-image");
    expect(test.createAnimatedRenderer).toHaveBeenCalledOnce();
    expect(test.animatedRenderer.load).toHaveBeenCalledWith(expect.objectContaining({
      kind: "animated-image",
      imageUrl: "asset://pet-user-1/body.png",
    }));
    expect(runtime.getSurface()).not.toBe(runtime.getHitSurface());
    expect(test.options.loadLive2DAsset).not.toHaveBeenCalled();
    expect(test.options.createLive2DRenderer).not.toHaveBeenCalled();
  });

  it("provides the real animated renderer with an offscreen compose surface", async () => {
    const contexts = Array.from({ length: 3 }, () => ({
      clearRect: vi.fn(),
      drawImage: vi.fn(),
      setTransform: vi.fn(),
      save: vi.fn(),
      restore: vi.fn(),
      translate: vi.fn(),
      rotate: vi.fn(),
    }));
    const surfaces = contexts.map((context) => ({
      className: "",
      width: 0,
      height: 0,
      style: {} as CSSStyleDeclaration,
      getContext: vi.fn(() => context),
      remove: vi.fn(),
    } as unknown as HTMLCanvasElement));
    const [displaySurface, hitSurface, composeSurface] = surfaces;
    const availableSurfaces = [...surfaces];
    const createCanvas = vi.fn(() => availableSurfaces.shift()!);
    const replaceChildren = vi.fn();
    const root = { replaceChildren } as unknown as HTMLElement;
    vi.stubGlobal("Image", class {
      readonly width = 1000;
      readonly height = 1000;
      crossOrigin = "";
      src = "";
      async decode(): Promise<void> {}
    });

    try {
      const runtime = await createPetRendererRuntime(
        "pet-user-1",
        validAnimatedManifest(),
        {
          root,
          createCanvas,
          loadAnimatedImageAsset: vi.fn(async () => ({
            kind: "animated-image" as const,
            imageUrl: "asset://pet-user-1/body.png",
            motionProfile: validMotionProfile(),
          })),
        },
      );

      expect(runtime.kind()).toBe("animated-image");
      expect(runtime.getSurface()).toBe(displaySurface);
      expect(runtime.getHitSurface()).toBe(hitSurface);
      expect(createCanvas).toHaveBeenCalledTimes(3);
      expect(replaceChildren).toHaveBeenCalledWith(displaySurface, hitSurface);
      expect(replaceChildren.mock.calls.every((children) =>
        !children.includes(composeSurface)
      )).toBe(true);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("falls back to the manifest preview when initial Live2D loading fails", async () => {
    const test = harness({ liveLoadError: new Error("webgl unavailable") });

    const runtime = await createPetRendererRuntime("pet-a", v2Manifest, test.options);

    expect(test.liveRenderer.destroy).toHaveBeenCalledOnce();
    expect(test.staticRenderers[0]?.load).toHaveBeenCalledWith({
      kind: "static-png",
      imageUrl: "asset://pet-a/preview.png",
    });
    expect(runtime.kind()).toBe("static-png");
    expect(test.diagnostics).toEqual([{
      petId: "pet-a",
      manifestVersion: 2,
      stage: "live2d-initial-load",
      message: "webgl unavailable",
    }]);
  });

  it("replaces Live2D with the preview after context restoration fails", async () => {
    const test = harness();
    const runtime = await createPetRendererRuntime("pet-a", v2Manifest, test.options);

    test.reload(new Error("restore failed"));
    await vi.waitFor(() => expect(runtime.kind()).toBe("static-png"));

    expect(test.staticRenderers[0]?.load).toHaveBeenCalledWith({
      kind: "static-png",
      imageUrl: "asset://pet-a/preview.png",
    });
    expect(test.liveRenderer.destroy).toHaveBeenCalledOnce();
    expect(test.diagnostics).toContainEqual({
      petId: "pet-a",
      manifestVersion: 2,
      stage: "live2d-context-restore",
      message: "restore failed",
    });
  });
});
