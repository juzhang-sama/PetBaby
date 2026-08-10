import { afterEach, describe, expect, it, vi } from "vitest";
import type { RuntimeAssetManifestV2 } from "../runtime-assets/live2d-manifest";
import type { PetRendererRuntime } from "./pet-renderer-bootstrap";
import type { MountedPetRuntime } from "./pet-runtime-slot";
import type { RuntimePetDescriptor } from "./pet-switch-protocol";
import { loadRuntimePet, type RuntimePetLoaderPorts } from "./runtime-pet-loader";
import {
  BUILTIN_PET_ID,
  finalizeStartupRecovery,
  loadStartupRuntime,
} from "./startup-runtime-recovery";

function runtime(petId: string): MountedPetRuntime {
  return {
    petId,
    host: { destroy: vi.fn() },
  } as unknown as MountedPetRuntime;
}

function live2dManifest(petId: string): RuntimeAssetManifestV2 {
  return {
    schemaVersion: 2,
    renderer: "live2d-v1",
    petId,
    variantId: "variant-live2d",
    modelEntry: "model.model3.json",
    previewImage: "preview.png",
    files: [
      { role: "model", relativePath: "model.model3.json", sha256: "a".repeat(64) },
      { role: "preview", relativePath: "preview.png", sha256: "b".repeat(64) },
    ],
    semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
    license: {
      id: "test",
      author: "test",
      source: "test",
      commercialUse: true,
      redistributable: false,
    },
  };
}

function staticRuntime(petId: string): MountedPetRuntime {
  const surface = {} as HTMLCanvasElement;
  return {
    petId,
    host: { destroy: vi.fn() } as unknown as PetRendererRuntime["host"],
    getSurface: () => surface,
    getHitSurface: () => surface,
    kind: () => "static-png",
    recoverToPreview: async () => undefined,
  };
}

describe("loadStartupRuntime", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("keeps a healthy active pet without persisting a fallback", async () => {
    const activeRuntime = runtime("pet-user-1");
    const prepare = vi.fn(async (petId: string) => ({ petId, source: "installed" }) as RuntimePetDescriptor);
    const load = vi.fn(async () => activeRuntime);
    const commit = vi.fn(async () => undefined);

    await expect(loadStartupRuntime("pet-user-1", { prepare, load, commit })).resolves.toEqual({
      runtime: activeRuntime,
      recoveredToBuiltin: false,
    });
    expect(prepare).toHaveBeenCalledTimes(1);
    expect(commit).not.toHaveBeenCalled();
  });

  it("recovers a broken user pet to the built-in pet and persists the repair", async () => {
    const builtinRuntime = runtime("pet-live2d-v1");
    const prepare = vi.fn(async (petId: string) => ({
      petId,
      source: petId === "pet-live2d-v1" ? "builtin" : "installed",
    }) as RuntimePetDescriptor);
    const load = vi.fn(async (descriptor: RuntimePetDescriptor) => {
      if (descriptor.petId === "pet-user-1") throw new Error("corrupt PNG");
      return builtinRuntime;
    });
    const commit = vi.fn(async () => undefined);

    await expect(loadStartupRuntime("pet-user-1", { prepare, load, commit })).resolves.toEqual({
      runtime: builtinRuntime,
      recoveredToBuiltin: true,
    });
    expect(prepare).toHaveBeenNthCalledWith(2, "pet-live2d-v1");
    expect(commit).toHaveBeenCalledWith("pet-live2d-v1");
  });

  it("recovers to the built-in pet when an installed Live2D runtime falls back to static", async () => {
    const installedFallback = staticRuntime("pet-user-1");
    const builtinFallback = staticRuntime("pet-live2d-v1");
    const ports: RuntimePetLoaderPorts = {
      readInstalledManifest: vi.fn(async () => live2dManifest("pet-user-1")),
      installedAssetUrl: vi.fn((petId, path) => `asset://${petId}/${path}`),
      createBuiltinTransport: vi.fn(() => ({
        readManifest: vi.fn(async () => live2dManifest("pet-live2d-v1")),
        readFile: vi.fn(),
      })),
      createRuntime: vi.fn(async (petId) => (
        petId === "pet-user-1" ? installedFallback : builtinFallback
      )),
      createPreviewRuntime: vi.fn(async () => builtinFallback),
    };
    vi.stubGlobal("window", { location: { origin: "http://localhost" } });
    const prepare = vi.fn(async (petId: string) => ({
      petId,
      source: petId === "pet-live2d-v1" ? "builtin" : "installed",
    }) as RuntimePetDescriptor);
    const commit = vi.fn(async () => undefined);

    const result = await loadStartupRuntime("pet-user-1", {
      prepare,
      load: (descriptor) => loadRuntimePet(
        descriptor,
        {} as HTMLElement,
        ports,
        { allowPreviewFallback: true },
      ),
      commit,
    });

    expect(result.recoveredToBuiltin).toBe(true);
    expect(result.runtime.petId).toBe("pet-live2d-v1");
    expect(result.runtime.host).toBe(builtinFallback.host);
    expect(installedFallback.host.destroy).toHaveBeenCalledOnce();
    expect(commit).toHaveBeenCalledWith("pet-live2d-v1");
  });

  it("does not retry when the built-in pet itself fails", async () => {
    const prepare = vi.fn(async (petId: string) => ({ petId, source: "builtin" }) as RuntimePetDescriptor);
    const load = vi.fn(async () => { throw new Error("built-in unavailable"); });
    const commit = vi.fn(async () => undefined);

    await expect(loadStartupRuntime("pet-live2d-v1", { prepare, load, commit })).rejects.toThrow(
      "built-in unavailable",
    );
    expect(prepare).toHaveBeenCalledTimes(1);
    expect(commit).not.toHaveBeenCalled();
  });

  it("destroys the fallback runtime when persisting recovery fails", async () => {
    const builtinRuntime = runtime("pet-live2d-v1");
    const prepare = vi.fn(async (petId: string) => ({ petId, source: "builtin" }) as RuntimePetDescriptor);
    const load = vi.fn(async (descriptor: RuntimePetDescriptor) => {
      if (descriptor.petId !== "pet-live2d-v1") throw new Error("corrupt PNG");
      return builtinRuntime;
    });
    const commit = vi.fn(async () => { throw new Error("database unavailable"); });

    await expect(loadStartupRuntime("pet-user-1", { prepare, load, commit })).rejects.toThrow(
      "database unavailable",
    );
    expect(builtinRuntime.host.destroy).toHaveBeenCalledOnce();
  });
});

describe("finalizeStartupRecovery", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  function finalizationPorts(options: {
    commitError?: Error;
    reconcileStatus?: "notCommitted" | "compensated" | "unknown";
    finishError?: Error;
    holdFinish?: boolean;
  } = {}) {
    const prepareSwitch = vi.fn(async () => undefined);
    const commit = vi.fn(async () => {
      if (options.commitError) throw options.commitError;
    });
    const reconcileCommit = vi.fn(async () => ({
      status: options.reconcileStatus ?? "notCommitted" as const,
    }));
    const cancel = vi.fn(async () => undefined);
    const finish = vi.fn(async () => {
      if (options.holdFinish) await new Promise<never>(() => undefined);
      if (options.finishError) throw options.finishError;
    });
    return { prepareSwitch, commit, reconcileCommit, cancel, finish };
  }

  it("reconciles a lost commit response and finishes only after DB compensation", async () => {
    const ports = finalizationPorts({
      commitError: new Error("commit response lost"),
      reconcileStatus: "compensated",
    });
    vi.stubGlobal("crypto", { randomUUID: () => "startup-response-lost" });

    await expect(finalizeStartupRecovery("pet-damaged", BUILTIN_PET_ID, ports, 50)).rejects.toThrow(
      "commit response lost",
    );

    expect(ports.reconcileCommit).toHaveBeenCalledOnce();
    expect(ports.finish).toHaveBeenCalledWith("startup-response-lost");
    expect(ports.cancel).not.toHaveBeenCalled();
  });

  it("leaves an unknown commit owner to TTL", async () => {
    const ports = finalizationPorts({
      commitError: new Error("commit response lost"),
      reconcileStatus: "unknown",
    });
    vi.stubGlobal("crypto", { randomUUID: () => "startup-unknown" });

    await expect(finalizeStartupRecovery("pet-damaged", BUILTIN_PET_ID, ports, 50)).rejects.toThrow(
      "commit response lost",
    );
    expect(ports.cancel).not.toHaveBeenCalled();
    expect(ports.finish).not.toHaveBeenCalled();
  });

  it("cancels only when reconciliation proves the startup commit did not land", async () => {
    const ports = finalizationPorts({
      commitError: new Error("commit rejected"),
      reconcileStatus: "notCommitted",
    });
    vi.stubGlobal("crypto", { randomUUID: () => "startup-not-committed" });

    await expect(finalizeStartupRecovery("pet-damaged", BUILTIN_PET_ID, ports, 50)).rejects.toThrow(
      "commit rejected",
    );
    expect(ports.cancel).toHaveBeenCalledWith("startup-not-committed");
    expect(ports.finish).not.toHaveBeenCalled();
  });

  it("keeps the confirmed builtin commit and returns a warning when finish rejects", async () => {
    const ports = finalizationPorts({ finishError: new Error("finish busy") });
    vi.stubGlobal("crypto", { randomUUID: () => "startup-finish-reject" });

    await expect(finalizeStartupRecovery("pet-damaged", BUILTIN_PET_ID, ports, 50)).resolves.toEqual({
      warning: expect.stringContaining("finish busy"),
    });
    expect(ports.cancel).not.toHaveBeenCalled();
    expect(ports.reconcileCommit).not.toHaveBeenCalled();
  });

  it("bounds a never-settling finish without cancelling the confirmed commit", async () => {
    vi.useFakeTimers();
    const ports = finalizationPorts({ holdFinish: true });
    vi.stubGlobal("crypto", { randomUUID: () => "startup-finish-timeout" });

    const finalizing = finalizeStartupRecovery("pet-damaged", BUILTIN_PET_ID, ports, 50);
    await vi.advanceTimersByTimeAsync(50);

    await expect(finalizing).resolves.toEqual({
      warning: expect.stringContaining("超时"),
    });
    expect(ports.cancel).not.toHaveBeenCalled();
  });
});
