import { afterEach, describe, expect, it, vi } from "vitest";
import type { PetRendererRuntime } from "./pet-renderer-bootstrap";
import type { PetSwitchRequest, RuntimePetDescriptor } from "./pet-switch-protocol";
import { PetRuntimeSlot, type MountedPetRuntime } from "./pet-runtime-slot";
import { PetSwitchCoordinator } from "./pet-switch-coordinator";
import type { PetRenderer } from "./pet-renderer";

function request(petId: string, requestId: string = crypto.randomUUID()): PetSwitchRequest {
  return { requestId, petId };
}

function fakeRuntime(petId: string): MountedPetRuntime {
  const host: PetRenderer = {
    load: vi.fn(async () => undefined),
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
  const runtime: PetRendererRuntime = {
    host: host as PetRendererRuntime["host"],
    getSurface: () => ({ dataset: { petId } }) as unknown as HTMLCanvasElement,
    kind: () => "static-png",
    recoverToPreview: async () => undefined,
  };
  return { ...runtime, petId };
}

interface HarnessOptions {
  loadError?: Error;
  commitError?: Error;
  holdLoad?: boolean;
  rollbackFailures?: number;
  destroyOldRuntimeError?: Error;
}

function coordinatorHarness(options: HarnessOptions = {}) {
  let replaceCalls = 0;
  const rendererRoot = {
    replaceChildren: vi.fn(() => {
      replaceCalls += 1;
      if (replaceCalls >= 3 && replaceCalls < 3 + (options.rollbackFailures ?? 0)) {
        throw new Error("rollback surface failed");
      }
    }),
  } as unknown as HTMLElement;
  const oldRuntime = fakeRuntime("pet-a");
  const candidate = fakeRuntime("pet-b");
  const slot = new PetRuntimeSlot(rendererRoot, oldRuntime);
  if (options.destroyOldRuntimeError) {
    vi.mocked(oldRuntime.host.destroy).mockImplementation(() => {
      throw options.destroyOldRuntimeError;
    });
  }
  const prepare = vi.fn(async (petId: string): Promise<RuntimePetDescriptor> => ({
    petId,
    source: "installed",
  }));
  const commitSelection = vi.fn(async () => {
    if (options.commitError) throw options.commitError;
  });
  const refreshHitRegion = vi.fn(async () => undefined);
  const probe = vi.fn();
  let resolveLoadStarted!: () => void;
  const loadStarted = new Promise<void>((resolve) => { resolveLoadStarted = resolve; });
  let releaseLoad!: () => void;
  const releasePromise = new Promise<void>((resolve) => { releaseLoad = resolve; });
  const load = vi.fn(async (_descriptor: RuntimePetDescriptor, _stagingRoot: HTMLElement) => {
    if (options.loadError) throw options.loadError;
    if (options.holdLoad) {
      resolveLoadStarted();
      await releasePromise;
    }
    return candidate;
  });
  const coordinator = new PetSwitchCoordinator(slot, {
    prepare,
    load,
    probe,
    commit: commitSelection,
    refreshHitRegion,
  });
  return {
    candidate,
    commitSelection,
    coordinator,
    load,
    loadStarted,
    oldRuntime,
    prepare,
    probe,
    refreshHitRegion,
    releaseLoad,
    rendererRoot,
    slot,
  };
}

describe("PetSwitchCoordinator", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("keeps the old runtime when candidate loading fails", async () => {
    const test = coordinatorHarness({ loadError: new Error("bad asset") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const result = await test.coordinator.switch(request("pet-b"));

    expect(result).toMatchObject({ ok: false, code: "load-failed" });
    expect(test.slot.activePetId).toBe("pet-a");
    expect(test.commitSelection).not.toHaveBeenCalled();
  });

  it("rolls back the visual swap when persistence fails", async () => {
    const test = coordinatorHarness({ commitError: new Error("sqlite busy") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const result = await test.coordinator.switch(request("pet-b"));

    expect(result).toMatchObject({ ok: false, code: "persist-failed" });
    expect(test.slot.activePetId).toBe("pet-a");
    expect(test.oldRuntime.host.destroy).not.toHaveBeenCalled();
  });

  it("rejects a concurrent request without disturbing the in-flight switch", async () => {
    const test = coordinatorHarness({ holdLoad: true });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const first = test.coordinator.switch(request("pet-b", "r1"));
    await test.loadStarted;
    await expect(test.coordinator.switch(request("pet-c", "r2"))).resolves.toMatchObject({
      ok: false,
      requestId: "r2",
      code: "request-stale",
    });
    test.releaseLoad();
    await expect(first).resolves.toMatchObject({ ok: true, petId: "pet-b" });
  });

  it("loads every candidate into a staging root separate from the mounted renderer root", async () => {
    const test = coordinatorHarness();
    const stagingRoot = {} as HTMLElement;
    vi.stubGlobal("document", { createElement: vi.fn(() => stagingRoot) });

    await expect(test.coordinator.switch(request("pet-b"))).resolves.toMatchObject({ ok: true });

    expect(test.load).toHaveBeenCalledWith(
      { petId: "pet-b", source: "installed" },
      stagingRoot,
    );
    expect(stagingRoot).not.toBe(test.rendererRoot);
  });

  it("reports success when old-runtime cleanup throws after the backend commit succeeded", async () => {
    const test = coordinatorHarness({ destroyOldRuntimeError: new Error("destroy failed") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b"))).resolves.toMatchObject({
      ok: true,
      petId: "pet-b",
    });
    expect(test.commitSelection).toHaveBeenCalledOnce();
  });

  it("retries a rollback failure and returns the original persistence failure", async () => {
    const test = coordinatorHarness({
      commitError: new Error("sqlite busy"),
      rollbackFailures: 1,
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b"))).resolves.toMatchObject({
      ok: false,
      code: "persist-failed",
    });
    expect(test.slot.activePetId).toBe("pet-a");
  });

  it("returns a deterministic failure when rollback cannot converge", async () => {
    const test = coordinatorHarness({
      commitError: new Error("sqlite busy"),
      rollbackFailures: Number.POSITIVE_INFINITY,
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b"))).resolves.toMatchObject({
      ok: false,
      code: "persist-failed",
    });
  });
});
