import { describe, expect, it, vi } from "vitest";
import type { MountedPetRuntime } from "./pet-runtime-slot";
import type { RuntimePetDescriptor } from "./pet-switch-protocol";
import { loadStartupRuntime } from "./startup-runtime-recovery";

function runtime(petId: string): MountedPetRuntime {
  return {
    petId,
    host: { destroy: vi.fn() },
  } as unknown as MountedPetRuntime;
}

describe("loadStartupRuntime", () => {
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
