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
    getHitSurface: () => ({ dataset: { petId: `${petId}-hit` } }) as unknown as HTMLCanvasElement,
    kind: () => "static-png",
    recoverToPreview: async () => undefined,
  };
  return { ...runtime, petId };
}

interface HarnessOptions {
  prepareError?: Error;
  loadError?: Error;
  probeError?: Error;
  commitError?: Error;
  candidateMotionError?: Error;
  holdLoad?: boolean;
  holdCommit?: boolean;
  rollbackFailures?: number;
  backendRollbackError?: Error;
  rollbackStatus?: "compensated" | "unknown";
  rollbackWarning?: string;
  finishError?: Error;
  fallbackCheckErrorAfterCommit?: Error;
  reconciliationStatus?: "notCommitted" | "compensated" | "unknown";
  destroyOldRuntimeError?: Error;
  abortNever?: boolean;
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
  if (options.candidateMotionError) {
    vi.mocked(candidate.host.playMotion).mockImplementation(() => {
      throw options.candidateMotionError;
    });
  }
  if (options.destroyOldRuntimeError) {
    vi.mocked(oldRuntime.host.destroy).mockImplementation(() => {
      throw options.destroyOldRuntimeError;
    });
  }
  const prepare = vi.fn(async (_requestId: string, petId: string): Promise<RuntimePetDescriptor> => ({
    petId,
    source: "installed",
  }));
  if (options.prepareError) prepare.mockRejectedValue(options.prepareError);
  const refreshHitRegion = vi.fn(async () => undefined);
  const rollbackCommit = vi.fn(async () => {
    if (options.backendRollbackError) throw options.backendRollbackError;
    return {
      status: options.rollbackStatus ?? "compensated" as const,
      warning: options.rollbackWarning,
    };
  });
  const cancel = vi.fn(async () => undefined);
  const abortCreation = vi.fn(async () => {
    if (options.abortNever) await new Promise<never>(() => undefined);
  });
  const finish = vi.fn(async () => {
    if (options.finishError) throw options.finishError;
  });
  const reconcileCommit = vi.fn(async () => ({
    status: options.reconciliationStatus ?? "notCommitted" as const,
  }));
  const probe = vi.fn(() => {
    if (options.probeError) throw options.probeError;
  });
  let resolveLoadStarted!: () => void;
  const loadStarted = new Promise<void>((resolve) => { resolveLoadStarted = resolve; });
  let releaseLoad!: () => void;
  const releasePromise = new Promise<void>((resolve) => { releaseLoad = resolve; });
  let resolveCommitStarted!: () => void;
  const commitStarted = new Promise<void>((resolve) => { resolveCommitStarted = resolve; });
  let releaseCommit!: () => void;
  const commitReleasePromise = new Promise<void>((resolve) => { releaseCommit = resolve; });
  let candidatePreviewFallback = false;
  let fallbackChecks = 0;
  candidate.isPreviewFallback = () => {
    fallbackChecks += 1;
    if (fallbackChecks > 1 && options.fallbackCheckErrorAfterCommit) {
      throw options.fallbackCheckErrorAfterCommit;
    }
    return candidatePreviewFallback;
  };
  const commitSelection = vi.fn(async () => {
    if (options.holdCommit) {
      resolveCommitStarted();
      await commitReleasePromise;
    }
    if (options.commitError) throw options.commitError;
  });
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
    rollbackCommit,
    abortCreation,
    cancel,
    finish,
    reconcileCommit,
    refreshHitRegion,
  });
  return {
    candidate,
    abortCreation,
    cancel,
    commitSelection,
    commitStarted,
    coordinator,
    finish,
    reconcileCommit,
    load,
    loadStarted,
    oldRuntime,
    prepare,
    probe,
    refreshHitRegion,
    releaseLoad,
    releaseCommit,
    rollbackCommit,
    rendererRoot,
    slot,
    triggerCandidatePreviewFallback: () => { candidatePreviewFallback = true; },
  };
}

describe("PetSwitchCoordinator", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("keeps the old runtime when candidate loading fails", async () => {
    const test = coordinatorHarness({ loadError: new Error("bad asset") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const result = await test.coordinator.switch(request("pet-b"));

    expect(result).toMatchObject({ ok: false, code: "load-failed" });
    expect(test.slot.activePetId).toBe("pet-a");
    expect(test.commitSelection).not.toHaveBeenCalled();
    expect(test.cancel).toHaveBeenCalledOnce();
    expect(test.finish).not.toHaveBeenCalled();
  });

  it("cancels exactly once when backend prepare fails", async () => {
    const test = coordinatorHarness({ prepareError: new Error("missing pet") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b", "r-prepare"))).resolves.toMatchObject({ ok: false });

    expect(test.prepare).toHaveBeenCalledWith("r-prepare", "pet-b");
    expect(test.cancel).toHaveBeenCalledWith("r-prepare");
    expect(test.cancel).toHaveBeenCalledOnce();
    expect(test.finish).not.toHaveBeenCalled();
  });

  it("cancels exactly once when surface probing fails", async () => {
    const test = coordinatorHarness({ probeError: new Error("blank-frame") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b", "r-probe"))).resolves.toMatchObject({
      ok: false,
      code: "blank-frame",
    });

    expect(test.cancel).toHaveBeenCalledWith("r-probe");
    expect(test.cancel).toHaveBeenCalledOnce();
    expect(test.finish).not.toHaveBeenCalled();
  });

  it("aborts the exact creation session before cancelling a failed creation request", async () => {
    const test = coordinatorHarness({ loadError: new Error("bad asset") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch({
      requestId: "r-creation-load",
      petId: "pet-b",
      acceptedVariantId: "variant-b",
      creationSessionId: "session-b",
    })).resolves.toMatchObject({ ok: false });

    expect(test.abortCreation).toHaveBeenCalledWith("session-b", "bad asset");
    expect(test.cancel).toHaveBeenCalledWith("r-creation-load");
    expect(test.abortCreation.mock.invocationCallOrder[0]).toBeLessThan(
      test.cancel.mock.invocationCallOrder[0]!,
    );
  });

  it("fact-checks a creation commit with abort before releasing an unknown request", async () => {
    const test = coordinatorHarness({
      commitError: new Error("transport state unknown"),
      reconciliationStatus: "unknown",
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch({
      requestId: "r-creation-unknown",
      petId: "pet-b",
      acceptedVariantId: "variant-b",
      creationSessionId: "session-b",
    })).resolves.toMatchObject({ ok: false, code: "persist-failed" });

    expect(test.abortCreation).toHaveBeenCalledWith("session-b", expect.stringContaining("unknown"));
    expect(test.cancel).toHaveBeenCalledWith("r-creation-unknown");
    expect(test.finish).not.toHaveBeenCalled();
  });

  it("rolls back the visual swap when persistence fails", async () => {
    const test = coordinatorHarness({ commitError: new Error("sqlite busy") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });
    test.slot.playMotion("idle", { priority: 10, loop: true });

    const result = await test.coordinator.switch(request("pet-b"));
    vi.mocked(test.oldRuntime.host.update).mockClear();
    test.slot.update(42);

    expect(result).toMatchObject({ ok: false, code: "persist-failed" });
    expect(test.slot.activePetId).toBe("pet-a");
    expect(test.candidate.host.playMotion).toHaveBeenCalledWith("idle", { priority: 10, loop: true });
    expect(test.candidate.host.destroy).toHaveBeenCalledOnce();
    expect(test.oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(test.oldRuntime.host.update).toHaveBeenCalledWith(42);
    expect(test.cancel).toHaveBeenCalledOnce();
    expect(test.finish).not.toHaveBeenCalled();
    expect(test.reconcileCommit).toHaveBeenCalledOnce();
    expect(test.rollbackCommit).not.toHaveBeenCalled();
  });

  it("finishes after commit rejection reconciliation confirms DB compensation", async () => {
    const test = coordinatorHarness({
      commitError: new Error("transport rejected after commit"),
      reconciliationStatus: "compensated",
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b", "r-commit-landed"))).resolves.toMatchObject({
      ok: false,
      code: "persist-failed",
    });

    expect(test.reconcileCommit).toHaveBeenCalledWith("pet-a", {
      requestId: "r-commit-landed",
      petId: "pet-b",
    });
    expect(test.finish).toHaveBeenCalledWith("r-commit-landed");
    expect(test.finish).toHaveBeenCalledOnce();
    expect(test.cancel).not.toHaveBeenCalled();
    expect(test.rollbackCommit).not.toHaveBeenCalled();
    expect(test.slot.activePetId).toBe("pet-a");
  });

  it("leaves owner to TTL when commit rejection reconciliation is unknown", async () => {
    const test = coordinatorHarness({
      commitError: new Error("transport state unknown"),
      reconciliationStatus: "unknown",
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b", "r-commit-unknown"))).resolves.toMatchObject({
      ok: false,
      code: "persist-failed",
    });

    expect(test.reconcileCommit).toHaveBeenCalledOnce();
    expect(test.finish).not.toHaveBeenCalled();
    expect(test.cancel).not.toHaveBeenCalled();
    expect(test.rollbackCommit).not.toHaveBeenCalled();
    expect(test.slot.activePetId).toBe("pet-a");
  });

  it("starts the carried idle after visual activation and before persistence commits", async () => {
    const test = coordinatorHarness({ holdCommit: true });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });
    test.slot.playMotion("idle", { priority: 10, loop: true });

    const switching = test.coordinator.switch(request("pet-b"));
    await test.commitStarted;

    expect(test.slot.activePetId).toBe("pet-b");
    expect(test.candidate.host.playMotion).toHaveBeenCalledWith("idle", { priority: 10, loop: true });

    test.releaseCommit();
    await expect(switching).resolves.toMatchObject({ ok: true, petId: "pet-b" });
    vi.mocked(test.candidate.host.update).mockClear();
    test.slot.update(42);

    expect(test.candidate.host.playMotion).toHaveBeenCalledOnce();
    expect(test.candidate.host.playMotion).toHaveBeenCalledWith("idle", { priority: 10, loop: true });
    expect(test.candidate.host.update).toHaveBeenCalledWith(42);
  });

  it("rolls back candidate idle startup failure before persistence", async () => {
    const test = coordinatorHarness({ candidateMotionError: new Error("idle failed") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });
    test.slot.playMotion("idle", { priority: 10, loop: true });

    const result = await test.coordinator.switch(request("pet-b"));
    vi.mocked(test.oldRuntime.host.update).mockClear();
    test.slot.update(42);

    expect(result).toMatchObject({ ok: false, code: "load-failed", message: "idle failed" });
    expect(test.commitSelection).not.toHaveBeenCalled();
    expect(test.slot.activePetId).toBe("pet-a");
    expect(test.candidate.host.destroy).toHaveBeenCalledOnce();
    expect(test.oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(test.oldRuntime.host.update).toHaveBeenCalledWith(42);
    expect(test.cancel).toHaveBeenCalledOnce();
    expect(test.finish).not.toHaveBeenCalled();
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
      warning: expect.stringContaining("destroy failed"),
    });
    expect(test.commitSelection).toHaveBeenCalledOnce();
    expect(test.finish).toHaveBeenCalledOnce();
    expect(test.cancel).not.toHaveBeenCalled();
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
      message: expect.stringContaining("rollback 未收敛"),
    });
  });

  it("compensates persistence and rolls back when a candidate falls back during a pending commit", async () => {
    const test = coordinatorHarness({ holdCommit: true });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const switching = test.coordinator.switch(request("pet-b", "r-fallback"));
    await test.commitStarted;
    test.triggerCandidatePreviewFallback();
    test.releaseCommit();

    await expect(switching).resolves.toMatchObject({
      ok: false,
      code: "load-failed",
      message: expect.stringContaining("预览帧"),
    });
    expect(test.slot.activePetId).toBe("pet-a");
    expect(test.rollbackCommit).toHaveBeenCalledWith("pet-a", {
      requestId: "r-fallback",
      petId: "pet-b",
    });
    expect(test.finish).toHaveBeenCalledWith("r-fallback");
    expect(test.cancel).not.toHaveBeenCalled();
    expect(test.oldRuntime.host.destroy).not.toHaveBeenCalled();
  });

  it("aborts then cancels a creation that falls back after its commit", async () => {
    const test = coordinatorHarness({ holdCommit: true });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });
    const creationRequest = {
      requestId: "r-creation-fallback",
      petId: "pet-b",
      acceptedVariantId: "candidate-b",
      creationSessionId: "session-b",
    };

    const switching = test.coordinator.switch(creationRequest);
    await test.commitStarted;
    test.triggerCandidatePreviewFallback();
    test.releaseCommit();
    await expect(switching).resolves.toMatchObject({ ok: false, code: "load-failed" });

    expect(test.rollbackCommit).toHaveBeenCalledWith("pet-a", creationRequest);
    expect(test.abortCreation).toHaveBeenCalledWith("session-b", expect.stringContaining("预览"));
    expect(test.cancel).toHaveBeenCalledWith("r-creation-fallback");
    expect(test.finish).not.toHaveBeenCalled();
  });

  it("bounds a hung creation abort and still cancels the exact owner", async () => {
    vi.useFakeTimers();
    const test = coordinatorHarness({ loadError: new Error("bad asset"), abortNever: true });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });
    const switching = test.coordinator.switch({
      requestId: "r-hung-abort",
      petId: "pet-b",
      creationSessionId: "session-b",
    });
    await vi.waitFor(() => expect(test.abortCreation).toHaveBeenCalledOnce());
    await vi.advanceTimersByTimeAsync(2_000);

    await expect(switching).resolves.toMatchObject({ ok: false });
    expect(test.cancel).toHaveBeenCalledWith("r-hung-abort");
  });

  it("reports a visible warning without rolling back when finish rejects after commit", async () => {
    const test = coordinatorHarness({ finishError: new Error("mutation owner busy") });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b", "r-finish-busy"))).resolves.toMatchObject({
      ok: true,
      petId: "pet-b",
      warning: expect.stringContaining("mutation owner busy"),
    });
    expect(test.slot.activePetId).toBe("pet-b");
    expect(test.cancel).not.toHaveBeenCalled();
    expect(test.rollbackCommit).not.toHaveBeenCalled();
  });

  it("leaves the lease to TTL when committed persistence compensation cannot converge", async () => {
    const test = coordinatorHarness({
      holdCommit: true,
      backendRollbackError: new Error("database unavailable"),
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const switching = test.coordinator.switch(request("pet-b", "r-uncertain"));
    await test.commitStarted;
    test.triggerCandidatePreviewFallback();
    test.releaseCommit();
    await expect(switching).resolves.toMatchObject({ ok: false });

    expect(test.rollbackCommit).toHaveBeenCalledTimes(2);
    expect(test.finish).not.toHaveBeenCalled();
    expect(test.cancel).not.toHaveBeenCalled();
  });

  it("finishes a DB-compensated rollback and exposes its session warning", async () => {
    const test = coordinatorHarness({
      holdCommit: true,
      rollbackStatus: "compensated",
      rollbackWarning: "session lock poisoned",
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const switching = test.coordinator.switch(request("pet-b", "r-compensated-warning"));
    await test.commitStarted;
    test.triggerCandidatePreviewFallback();
    test.releaseCommit();

    await expect(switching).resolves.toMatchObject({
      ok: false,
      message: expect.stringContaining("session lock poisoned"),
    });
    expect(test.finish).toHaveBeenCalledOnce();
    expect(test.cancel).not.toHaveBeenCalled();
  });

  it("leaves the owner to TTL after two explicit unknown rollback results", async () => {
    const test = coordinatorHarness({ holdCommit: true, rollbackStatus: "unknown" });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    const switching = test.coordinator.switch(request("pet-b", "r-rollback-unknown"));
    await test.commitStarted;
    test.triggerCandidatePreviewFallback();
    test.releaseCommit();
    await expect(switching).resolves.toMatchObject({ ok: false });

    expect(test.rollbackCommit).toHaveBeenCalledTimes(2);
    expect(test.finish).not.toHaveBeenCalled();
    expect(test.cancel).not.toHaveBeenCalled();
  });

  it("adds finalization failure to a compensated error instead of cancelling", async () => {
    const test = coordinatorHarness({
      fallbackCheckErrorAfterCommit: new Error("fallback state unavailable"),
      finishError: new Error("finish transport failed"),
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b", "r-compensated-finish"))).resolves.toMatchObject({
      ok: false,
      message: expect.stringContaining("finish transport failed"),
    });
    expect(test.cancel).not.toHaveBeenCalled();
  });

  it("compensates and finishes instead of cancelling after an unexpected post-commit failure", async () => {
    const test = coordinatorHarness({
      fallbackCheckErrorAfterCommit: new Error("fallback state unavailable"),
    });
    vi.stubGlobal("document", { createElement: vi.fn(() => ({}) as HTMLElement) });

    await expect(test.coordinator.switch(request("pet-b", "r-post-commit"))).resolves.toMatchObject({
      ok: false,
    });

    expect(test.rollbackCommit).toHaveBeenCalledWith("pet-a", {
      requestId: "r-post-commit",
      petId: "pet-b",
    });
    expect(test.finish).toHaveBeenCalledWith("r-post-commit");
    expect(test.cancel).not.toHaveBeenCalled();
  });
});
