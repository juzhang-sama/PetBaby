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
} {
  return {
    readInstalledManifest: vi.fn(async () => ({ source: "installed-manifest" })),
    createBuiltinTransport: vi.fn(() => ({
      readManifest: vi.fn(async () => ({ source: "builtin-manifest" })),
      readFile: vi.fn(),
    })),
    createRuntime: vi.fn(async () => fakeRuntime()),
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
    expect(ports.createBuiltinTransport).not.toHaveBeenCalled();
    expect(ports.createRuntime).toHaveBeenCalledWith(
      "pet-user-1",
      { source: "installed-manifest" },
      { root },
    );
  });
});
