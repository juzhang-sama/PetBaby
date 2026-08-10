import { afterEach, describe, expect, it, vi } from "vitest";
import type { PetRendererRuntime } from "./pet-renderer-bootstrap";
import { loadRuntimePet, type RuntimePetLoaderPorts } from "./runtime-pet-loader";

function fakeRuntime(): PetRendererRuntime {
  return {
    host: {} as PetRendererRuntime["host"],
    getSurface: () => ({}) as HTMLCanvasElement,
    kind: () => "static-png",
    recoverToPreview: async () => undefined,
  };
}

function loaderPorts(): RuntimePetLoaderPorts & {
  readInstalledManifest: ReturnType<typeof vi.fn>;
  createBuiltinTransport: ReturnType<typeof vi.fn>;
  createRuntime: ReturnType<typeof vi.fn>;
  createPreviewRuntime: ReturnType<typeof vi.fn>;
  installedAssetUrl: ReturnType<typeof vi.fn>;
} {
  return {
    readInstalledManifest: vi.fn(async () => ({ source: "installed-manifest" })),
    createBuiltinTransport: vi.fn(() => ({
      readManifest: vi.fn(async () => ({ source: "builtin-manifest" })),
      readFile: vi.fn(),
    })),
    createRuntime: vi.fn(async () => fakeRuntime()),
    createPreviewRuntime: vi.fn(async () => fakeRuntime()),
    installedAssetUrl: vi.fn((petId: string, relativePath: string) => (
      `http://pet-asset.localhost/${petId}/assets/${relativePath}`
    )),
  };
}

describe("loadRuntimePet", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("loads a built-in pet through its packaged transport without asking the backend for a manifest", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;
    vi.stubGlobal("window", { location: { origin: "http://localhost" } });

    const runtime = await loadRuntimePet({ petId: "pet-live2d-v1", source: "builtin" }, root, ports);

    expect(runtime.petId).toBe("pet-live2d-v1");
    expect(ports.readInstalledManifest).not.toHaveBeenCalled();
    expect(ports.createBuiltinTransport).toHaveBeenCalledWith(
      "/builtin-pets/pet-live2d-v1/manifest.json",
    );
    expect(ports.createRuntime).toHaveBeenCalledWith(
      "pet-live2d-v1",
      { source: "builtin-manifest" },
      expect.objectContaining({ root }),
    );
  });

  it("loads an installed pet from the backend without accessing the built-in package", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;

    const runtime = await loadRuntimePet({ petId: "pet-user-1", source: "installed" }, root, ports);

    expect(runtime.petId).toBe("pet-user-1");
    expect(ports.readInstalledManifest).toHaveBeenCalledWith("pet-user-1");
    expect(ports.installedAssetUrl).toHaveBeenCalledWith("pet-user-1", "body.png");
    expect(ports.createBuiltinTransport).not.toHaveBeenCalled();
    expect(ports.createRuntime).toHaveBeenCalledWith(
      "pet-user-1",
      { source: "installed-manifest" },
      expect.objectContaining({ root }),
    );
  });

  it("uses the initial built-in preview after manifest loading fails and reports the diagnostic", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;
    const previewRuntime = fakeRuntime();
    const diagnose = vi.fn();
    const createPreviewRuntime = vi.fn(async () => previewRuntime);
    ports.createBuiltinTransport.mockReturnValue({
      readManifest: vi.fn(async () => { throw new Error("missing manifest"); }),
      readFile: vi.fn(),
    });
    (ports as unknown as { createPreviewRuntime: typeof createPreviewRuntime }).createPreviewRuntime = createPreviewRuntime;
    vi.stubGlobal("window", { location: { origin: "http://localhost" } });
    const loadInitialRuntime = loadRuntimePet as unknown as (
      descriptor: { petId: string; source: "builtin" },
      target: HTMLElement,
      injectedPorts: typeof ports,
      options: { allowPreviewFallback: boolean; diagnose: typeof diagnose },
    ) => Promise<{ petId: string }>;

    await expect(loadInitialRuntime(
      { petId: "pet-live2d-v1", source: "builtin" },
      root,
      ports,
      { allowPreviewFallback: true, diagnose },
    )).resolves.toMatchObject({ petId: "pet-live2d-v1" });

    expect(createPreviewRuntime).toHaveBeenCalledWith(
      "/builtin-pets/pet-live2d-v1/preview.png",
      expect.objectContaining({ root }),
    );
    expect(diagnose).toHaveBeenCalledWith(expect.objectContaining({
      petId: "pet-live2d-v1",
      stage: "manifest-load",
    }));
  });

  it("uses the installed body preview after initial manifest loading fails", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;
    const createPreviewRuntime = vi.fn(async () => fakeRuntime());
    ports.readInstalledManifest.mockRejectedValue(new Error("missing manifest"));
    (ports as unknown as { createPreviewRuntime: typeof createPreviewRuntime }).createPreviewRuntime = createPreviewRuntime;
    const loadInitialRuntime = loadRuntimePet as unknown as (
      descriptor: { petId: string; source: "installed" },
      target: HTMLElement,
      injectedPorts: typeof ports,
      options: { allowPreviewFallback: boolean },
    ) => Promise<{ petId: string }>;

    await expect(loadInitialRuntime(
      { petId: "pet-user-1", source: "installed" },
      root,
      ports,
      { allowPreviewFallback: true },
    )).resolves.toMatchObject({ petId: "pet-user-1" });

    expect(createPreviewRuntime).toHaveBeenCalledWith(
      "http://pet-asset.localhost/pet-user-1/assets/body.png",
      expect.objectContaining({ root }),
    );
    expect(ports.installedAssetUrl).toHaveBeenCalledWith("pet-user-1", "body.png");
  });

  it("rejects an installed v3 runtime failure instead of falling back to a static preview", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;
    ports.readInstalledManifest.mockResolvedValue({ schemaVersion: 3 });
    ports.createRuntime.mockRejectedValue(new Error("animated-image renderer runtime is not available"));

    await expect(loadRuntimePet(
      { petId: "pet-user-1", source: "installed" },
      root,
      ports,
      { allowPreviewFallback: true },
    )).rejects.toThrow("animated-image renderer runtime is not available");

    expect(ports.createPreviewRuntime).not.toHaveBeenCalled();
  });

  it("forwards diagnostics and active-surface callbacks to the renderer runtime", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;
    const diagnose = vi.fn();
    const onSurfaceChanged = vi.fn();
    const loadWithLifecycle = loadRuntimePet as unknown as (
      descriptor: { petId: string; source: "installed" },
      target: HTMLElement,
      injectedPorts: typeof ports,
      options: { diagnose: typeof diagnose; onSurfaceChanged: typeof onSurfaceChanged },
    ) => Promise<{ petId: string }>;

    await loadWithLifecycle(
      { petId: "pet-user-1", source: "installed" },
      root,
      ports,
      { diagnose, onSurfaceChanged },
    );

    expect(ports.createRuntime).toHaveBeenCalledWith(
      "pet-user-1",
      { source: "installed-manifest" },
      expect.objectContaining({ root, diagnose, onSurfaceChanged: expect.any(Function) }),
    );
    const runtimeOptions = ports.createRuntime.mock.calls[0]?.[2];
    await runtimeOptions?.onSurfaceChanged?.();
    expect(onSurfaceChanged).toHaveBeenCalledOnce();
  });

  it("does not turn a hot-switch candidate load failure into a preview success", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;
    ports.createRuntime.mockRejectedValue(new Error("corrupt candidate"));

    await expect(loadRuntimePet(
      { petId: "pet-user-1", source: "installed" },
      root,
      ports,
    )).rejects.toThrow("corrupt candidate");

    expect(ports.createPreviewRuntime).not.toHaveBeenCalled();
  });

  it("rejects a Live2D candidate that internally fell back to a preview runtime", async () => {
    const ports = loaderPorts();
    const root = {} as HTMLElement;
    ports.readInstalledManifest.mockResolvedValue({ schemaVersion: 2 });
    const fallbackRuntime = fakeRuntime();
    fallbackRuntime.host = { destroy: vi.fn() } as unknown as PetRendererRuntime["host"];
    ports.createRuntime.mockResolvedValue(fallbackRuntime);

    await expect(loadRuntimePet(
      { petId: "pet-user-1", source: "installed" },
      root,
      ports,
    )).rejects.toThrow("preview fallback is not allowed for hot switching");

    expect(ports.createPreviewRuntime).not.toHaveBeenCalled();
  });
});
