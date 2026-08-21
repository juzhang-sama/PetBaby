import { describe, expect, it, vi } from "vitest";
import type { PetRendererHost } from "./pet-renderer-host";
import type { PetRendererRuntime } from "./pet-renderer-bootstrap";
import { DEFAULT_PET_CALIBRATION, type PetCalibrationV1 } from "./pet-calibration";
import type { PetExpression, PetHitArea, PetMotion, PetMotionHandle, PetRenderAsset, PetRenderer } from "./pet-renderer";
import { PetRuntimeSlot, type MountedPetRuntime } from "./pet-runtime-slot";
import { assertVisiblePixels } from "./render-surface-probe";

function calibration(overrides: Partial<PetCalibrationV1> = {}): PetCalibrationV1 {
  return { ...DEFAULT_PET_CALIBRATION, ...overrides };
}

function fakeRoot(): HTMLElement {
  return { replaceChildren: vi.fn() } as unknown as HTMLElement;
}

function fakeRuntime(petId: string): MountedPetRuntime {
  const surface = { dataset: { petId } } as unknown as HTMLCanvasElement;
  const hitSurface = { dataset: { petId: `${petId}-hit` } } as unknown as HTMLCanvasElement;
  const host: PetRenderer = {
    load: vi.fn(async (_asset: PetRenderAsset) => undefined),
    resize: vi.fn(),
    playMotion: vi.fn((_motion: PetMotion, _options?: { loop?: boolean; priority?: number }): PetMotionHandle => ({ cancel: vi.fn() })),
    setExpression: vi.fn((_expression: PetExpression, _weight?: number) => undefined),
    setLookTarget: vi.fn((_target: { x: number; y: number } | null) => undefined),
    setLipSync: vi.fn((_value: number) => undefined),
    setCalibration: vi.fn((_value: PetCalibrationV1) => undefined),
    hitTest: vi.fn((_point: { x: number; y: number }): PetHitArea | null => null),
    setVisibility: vi.fn((_visible: boolean) => undefined),
    update: vi.fn((_deltaMs: number) => undefined),
    destroy: vi.fn(),
  };
  const runtime: PetRendererRuntime = {
    host: host as PetRendererHost,
    getSurface: () => surface,
    getHitSurface: () => hitSurface,
    kind: () => "static-png",
    recoverToPreview: async () => undefined,
  };
  return { ...runtime, petId };
}

describe("PetRuntimeSlot", () => {
  it("keeps display and hit surfaces aligned with the active runtime", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);

    expect(slot.getSurface()).toBe(oldRuntime.getSurface());
    expect(slot.getHitSurface()).toBe(oldRuntime.getHitSurface());

    const swap = slot.prepare(candidate);
    expect(slot.getHitSurface()).toBe(oldRuntime.getHitSurface());
    swap.activate();
    expect(slot.getSurface()).toBe(candidate.getSurface());
    expect(slot.getHitSurface()).toBe(candidate.getHitSurface());

    swap.rollback();
    expect(slot.getSurface()).toBe(oldRuntime.getSurface());
    expect(slot.getHitSurface()).toBe(oldRuntime.getHitSurface());

    slot.destroy();
    expect(() => slot.getHitSurface()).toThrow("PetRuntimeSlot has been destroyed");
  });

  it("rolls back an activated candidate without destroying the previous runtime", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(root, oldRuntime);
    slot.resize({ width: 420, height: 520, dpr: 1 });
    slot.setVisibility(true);

    const swap = slot.prepare(candidate);
    swap.activate();
    expect(root.replaceChildren).toHaveBeenLastCalledWith(candidate.getSurface());
    swap.rollback();

    expect(root.replaceChildren).toHaveBeenLastCalledWith(oldRuntime.getSurface());
    expect(oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldRuntime.host.setVisibility).toHaveBeenLastCalledWith(true);
  });

  it("destroys the previous runtime only after commit", () => {
    const slot = new PetRuntimeSlot(fakeRoot(), fakeRuntime("old"));
    const swap = slot.prepare(fakeRuntime("next"));
    swap.activate();
    expect(slot.activePetId).toBe("next");
    swap.commit();
    expect(swap.previous.host.destroy).toHaveBeenCalledOnce();
  });

  it("continues looping idle on the activated candidate before updating it", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });

    const swap = slot.prepare(candidate);
    vi.mocked(candidate.host.update).mockClear();
    expect(candidate.host.playMotion).not.toHaveBeenCalled();

    swap.activate();
    slot.update(42);

    expect(candidate.host.playMotion).toHaveBeenCalledOnce();
    expect(candidate.host.playMotion).toHaveBeenCalledWith("idle", { priority: 10, loop: true });
    expect(candidate.host.update).toHaveBeenCalledOnce();
    expect(candidate.host.update).toHaveBeenCalledWith(42);
    expect(vi.mocked(candidate.host.playMotion).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(candidate.host.update).mock.invocationCallOrder[0]!);
    swap.commit();
    idle.cancel();
    expect(candidateIdle.cancel).toHaveBeenCalledOnce();
    expect(oldIdle.cancel).not.toHaveBeenCalled();
  });

  it("rolls back an activated candidate without interrupting the previous idle", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const previousIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(previousIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });

    const swap = slot.prepare(candidate);
    swap.activate();
    swap.rollback();
    vi.mocked(oldRuntime.host.update).mockClear();
    slot.update(42);

    expect(candidate.host.playMotion).toHaveBeenCalledWith("idle", { priority: 10, loop: true });
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(previousIdle.cancel).not.toHaveBeenCalled();
    expect(oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(oldRuntime.host.update).toHaveBeenCalledOnce();
    expect(oldRuntime.host.update).toHaveBeenCalledWith(42);
    idle.cancel();
    expect(previousIdle.cancel).toHaveBeenCalledOnce();
    expect(candidateIdle.cancel).not.toHaveBeenCalled();
  });

  it("rolls back when starting candidate idle fails", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const previousIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(previousIdle);
    vi.mocked(candidate.host.playMotion).mockImplementation(() => {
      throw new Error("idle failed");
    });
    const slot = new PetRuntimeSlot(root, oldRuntime);
    slot.setVisibility(true);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    const swap = slot.prepare(candidate);

    expect(() => swap.activate()).toThrow("idle failed");

    expect(slot.activePetId).toBe("old");
    expect(root.replaceChildren).toHaveBeenLastCalledWith(oldRuntime.getSurface());
    expect(oldRuntime.host.setVisibility).toHaveBeenLastCalledWith(true);
    expect(previousIdle.cancel).not.toHaveBeenCalled();
    expect(oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    swap.rollback();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    idle.cancel();
    expect(previousIdle.cancel).toHaveBeenCalledOnce();
  });

  it("cancels both runtime idle owners between activation and commit", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    const swap = slot.prepare(candidate);
    swap.activate();

    idle.cancel();
    swap.commit();
    idle.cancel();

    expect(oldIdle.cancel).toHaveBeenCalledOnce();
    expect(candidateIdle.cancel).toHaveBeenCalledOnce();
    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidate.host.destroy).not.toHaveBeenCalled();
  });

  it("keeps explicit idle cancellation after an activated swap rolls back", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    const swap = slot.prepare(candidate);
    swap.activate();

    idle.cancel();
    swap.rollback();
    idle.cancel();

    expect(oldIdle.cancel).toHaveBeenCalledOnce();
    expect(candidateIdle.cancel).toHaveBeenCalledOnce();
    expect(oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
  });

  it("does not cancel destroyed owners again when destroyed during an activated swap", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    slot.prepare(candidate).activate();

    slot.destroy();
    idle.cancel();

    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldIdle.cancel).not.toHaveBeenCalled();
    expect(candidateIdle.cancel).not.toHaveBeenCalled();
  });

  it("does not cancel the committed owner again after slot destruction", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    const swap = slot.prepare(candidate);
    swap.activate();
    swap.commit();

    slot.destroy();
    idle.cancel();

    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldIdle.cancel).not.toHaveBeenCalled();
    expect(candidateIdle.cancel).not.toHaveBeenCalled();
  });

  it("destroys the pending candidate and clears idle owners when previous destruction fails", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(oldRuntime.host.destroy).mockImplementation(() => {
      throw new Error("old destroy failed");
    });
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    slot.prepare(candidate);

    expect(() => slot.destroy()).toThrow("old destroy failed");
    idle.cancel();

    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldIdle.cancel).not.toHaveBeenCalled();
  });

  it("destroys the pending previous runtime and clears owners when candidate destruction fails", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    vi.mocked(candidate.host.destroy).mockImplementation(() => {
      throw new Error("candidate destroy failed");
    });
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    slot.prepare(candidate).activate();

    expect(() => slot.destroy()).toThrow("candidate destroy failed");
    idle.cancel();

    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidateIdle.cancel).not.toHaveBeenCalled();
    expect(oldIdle.cancel).not.toHaveBeenCalled();
  });

  it("attempts every pending destroy once and reports the first failure", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    vi.mocked(candidate.host.destroy).mockImplementation(() => {
      throw new Error("candidate destroy failed first");
    });
    vi.mocked(oldRuntime.host.destroy).mockImplementation(() => {
      throw new Error("old destroy failed second");
    });
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    slot.prepare(candidate).activate();

    expect(() => slot.destroy()).toThrow("candidate destroy failed first");
    idle.cancel();

    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidateIdle.cancel).not.toHaveBeenCalled();
    expect(oldIdle.cancel).not.toHaveBeenCalled();
  });

  it("forgets a failed previous cleanup after commit so cancellation only reaches the candidate", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const oldIdle = { cancel: vi.fn() };
    const candidateIdle = { cancel: vi.fn() };
    vi.mocked(oldRuntime.host.playMotion).mockReturnValue(oldIdle);
    vi.mocked(candidate.host.playMotion).mockReturnValue(candidateIdle);
    vi.mocked(oldRuntime.host.destroy).mockImplementation(() => {
      throw new Error("old cleanup failed");
    });
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const idle = slot.playMotion("idle", { priority: 10, loop: true });
    const swap = slot.prepare(candidate);
    swap.activate();

    expect(() => swap.commit()).toThrow("old cleanup failed");
    idle.cancel();
    idle.cancel();

    expect(slot.activePetId).toBe("candidate");
    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(oldIdle.cancel).not.toHaveBeenCalled();
    expect(candidateIdle.cancel).toHaveBeenCalledOnce();
    expect(candidate.host.destroy).not.toHaveBeenCalled();
  });

  it("prepares the candidate with the current viewport and visibility before activation", () => {
    const slot = new PetRuntimeSlot(fakeRoot(), fakeRuntime("old"));
    const candidate = fakeRuntime("next");
    slot.resize({ width: 420, height: 520, dpr: 2 });
    slot.setVisibility(true);

    slot.prepare(candidate);

    expect(candidate.host.resize).toHaveBeenCalledWith({ width: 420, height: 520, dpr: 2 });
    expect(candidate.host.setVisibility).toHaveBeenCalledWith(true);
    expect(candidate.host.update).toHaveBeenCalledWith(0);
  });

  it("rejects the active runtime as a candidate without destroying it", () => {
    const active = fakeRuntime("active");
    const slot = new PetRuntimeSlot(fakeRoot(), active);

    expect(() => slot.prepare(active)).toThrow("candidate is already active");

    expect(active.host.destroy).not.toHaveBeenCalled();
    expect(slot.activePetId).toBe("active");
  });

  it("rejects overlapping prepares and ignores settled transaction operations", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const overlapping = fakeRuntime("overlapping");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const first = slot.prepare(candidate);

    expect(() => slot.prepare(overlapping)).toThrow("swap is already pending");
    expect(overlapping.host.destroy).toHaveBeenCalledOnce();

    first.activate();
    first.commit();
    first.rollback();
    first.commit();

    expect(slot.activePetId).toBe("candidate");
    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidate.host.destroy).not.toHaveBeenCalled();
  });

  it("rejects a repeated activation without changing the active runtime", () => {
    const slot = new PetRuntimeSlot(fakeRoot(), fakeRuntime("old"));
    const swap = slot.prepare(fakeRuntime("candidate"));
    swap.activate();

    expect(() => swap.activate()).toThrow("swap is not activatable");
    expect(slot.activePetId).toBe("candidate");
  });

  it("destroys a candidate when preparation throws without changing the old runtime", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    vi.mocked(candidate.host.resize).mockImplementation(() => {
      throw new Error("resize failed");
    });
    const slot = new PetRuntimeSlot(root, oldRuntime);
    slot.resize({ width: 420, height: 520, dpr: 1 });

    expect(() => slot.prepare(candidate)).toThrow("resize failed");

    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(slot.activePetId).toBe("old");
    expect(root.replaceChildren).toHaveBeenLastCalledWith(oldRuntime.getSurface());
  });

  it("rolls back a prepared candidate when the first-frame probe fails", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const swap = slot.prepare(candidate);

    expect(() => assertVisiblePixels(new Uint8ClampedArray(4))).toThrow("blank-frame");
    swap.rollback();

    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(oldRuntime.host.destroy).not.toHaveBeenCalled();
    expect(slot.activePetId).toBe("old");
  });

  it("destroys once and rejects new renderer work after destruction", () => {
    const active = fakeRuntime("active");
    const slot = new PetRuntimeSlot(fakeRoot(), active);

    slot.destroy();
    slot.destroy();

    expect(active.host.destroy).toHaveBeenCalledOnce();
    expect(() => slot.prepare(fakeRuntime("candidate"))).toThrow("PetRuntimeSlot has been destroyed");
    expect(() => slot.update(16)).toThrow("PetRuntimeSlot has been destroyed");
  });

  it("keeps a pending unactivated candidate intact when it is prepared again", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const swap = slot.prepare(candidate);

    expect(() => slot.prepare(candidate)).toThrow("swap is already pending");
    expect(candidate.host.destroy).not.toHaveBeenCalled();

    swap.activate();
    swap.commit();
    expect(oldRuntime.host.destroy).toHaveBeenCalledOnce();
    expect(candidate.host.destroy).not.toHaveBeenCalled();
    expect(slot.activePetId).toBe("candidate");
  });

  it("keeps a pending previous runtime intact when preparation is attempted after activation", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const swap = slot.prepare(candidate);
    swap.activate();

    expect(() => slot.prepare(oldRuntime)).toThrow("swap is already pending");
    expect(oldRuntime.host.destroy).not.toHaveBeenCalled();

    swap.rollback();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    expect(slot.activePetId).toBe("old");
  });

  it("restores the old surface and cleans up the candidate when mounting it fails", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(root, oldRuntime);
    slot.setVisibility(true);
    vi.mocked(root.replaceChildren).mockImplementationOnce(() => {
      throw new Error("mount failed");
    });
    const swap = slot.prepare(candidate);

    expect(() => swap.activate()).toThrow("mount failed");

    expect(slot.activePetId).toBe("old");
    expect(root.replaceChildren).toHaveBeenLastCalledWith(oldRuntime.getSurface());
    expect(oldRuntime.host.setVisibility).toHaveBeenLastCalledWith(true);
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    swap.rollback();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
  });

  it("cleans up the candidate when hiding the previous runtime fails during activation", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(root, oldRuntime);
    slot.setVisibility(true);
    vi.mocked(oldRuntime.host.setVisibility).mockImplementationOnce(() => {
      throw new Error("hide failed");
    });
    const swap = slot.prepare(candidate);

    expect(() => swap.activate()).toThrow("hide failed");

    expect(slot.activePetId).toBe("old");
    expect(root.replaceChildren).toHaveBeenLastCalledWith(oldRuntime.getSurface());
    expect(oldRuntime.host.setVisibility).toHaveBeenLastCalledWith(true);
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
    swap.rollback();
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
  });

  it("keeps rollback retryable when restoring the previous surface fails", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(root, oldRuntime);
    const swap = slot.prepare(candidate);
    swap.activate();
    vi.mocked(root.replaceChildren).mockImplementationOnce(() => {
      throw new Error("restore failed");
    });

    expect(() => swap.rollback()).toThrow("restore failed");
    expect(slot.activePetId).toBe("candidate");
    expect(candidate.host.destroy).not.toHaveBeenCalled();

    swap.rollback();
    expect(slot.activePetId).toBe("old");
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
  });

  it("keeps rollback retryable when restoring visibility or destroying the candidate fails", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const swap = slot.prepare(candidate);
    swap.activate();
    vi.mocked(oldRuntime.host.setVisibility).mockImplementationOnce(() => {
      throw new Error("visibility failed");
    });

    expect(() => swap.rollback()).toThrow("visibility failed");
    expect(slot.activePetId).toBe("old");
    expect(candidate.host.destroy).not.toHaveBeenCalled();

    vi.mocked(candidate.host.destroy).mockImplementationOnce(() => {
      throw new Error("destroy failed");
    });
    expect(() => swap.rollback()).toThrow("destroy failed");
    expect(candidate.host.destroy).toHaveBeenCalledOnce();

    swap.rollback();
    expect(candidate.host.destroy).toHaveBeenCalledTimes(2);
  });

  it("restores the latest viewport to the previous runtime during rollback", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const swap = slot.prepare(candidate);
    swap.activate();
    slot.resize({ width: 640, height: 480, dpr: 2 });

    swap.rollback();

    expect(oldRuntime.host.resize).toHaveBeenLastCalledWith({ width: 640, height: 480, dpr: 2 });
  });

  it("delegates renderer calls to the active runtime before and after activation", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);

    slot.setLipSync(0.25);
    const swap = slot.prepare(candidate);
    swap.activate();
    slot.setLipSync(0.75);

    expect(oldRuntime.host.setLipSync).toHaveBeenCalledWith(0.25);
    expect(candidate.host.setLipSync).toHaveBeenCalledWith(0.75);
  });

  it("keeps calibration scoped to each runtime across rollback", () => {
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(fakeRoot(), oldRuntime);
    const oldValue = calibration({ breathAmplitudePercent: 1 });
    const candidateValue = calibration({ breathAmplitudePercent: 5 });

    slot.setCalibration(oldValue);
    const swap = slot.prepare(candidate);

    expect(candidate.host.setCalibration).not.toHaveBeenCalled();

    swap.activate();
    slot.setCalibration(candidateValue);
    swap.rollback();

    expect(oldRuntime.host.setCalibration).toHaveBeenCalledTimes(1);
    expect(oldRuntime.host.setCalibration).toHaveBeenCalledWith(oldValue);
    expect(candidate.host.setCalibration).toHaveBeenCalledWith(candidateValue);
  });

  it("keeps an activation failure pending until rollback restores a failed surface compensation", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(root, oldRuntime);
    slot.setVisibility(true);
    vi.mocked(root.replaceChildren)
      .mockImplementationOnce(() => {
        throw new Error("mount failed");
      })
      .mockImplementationOnce(() => {
        throw new Error("surface restore failed");
      });
    const swap = slot.prepare(candidate);

    expect(() => swap.activate()).toThrow("mount failed");
    expect(slot.activePetId).toBe("old");
    expect(candidate.host.destroy).not.toHaveBeenCalled();
    expect(() => swap.activate()).toThrow("swap is not activatable");
    expect(() => swap.commit()).toThrow("swap is not committable");

    swap.rollback();
    expect(root.replaceChildren).toHaveBeenLastCalledWith(oldRuntime.getSurface());
    expect(oldRuntime.host.setVisibility).toHaveBeenLastCalledWith(true);
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
  });

  it("keeps an activation failure pending until rollback restores failed visibility", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(root, oldRuntime);
    slot.setVisibility(true);
    vi.mocked(root.replaceChildren).mockImplementationOnce(() => {
      throw new Error("mount failed");
    });
    vi.mocked(oldRuntime.host.setVisibility)
      .mockImplementationOnce(() => undefined)
      .mockImplementationOnce(() => {
        throw new Error("visibility restore failed");
      });
    const swap = slot.prepare(candidate);

    expect(() => swap.activate()).toThrow("mount failed");
    expect(candidate.host.destroy).not.toHaveBeenCalled();

    swap.rollback();
    expect(oldRuntime.host.setVisibility).toHaveBeenLastCalledWith(true);
    expect(candidate.host.destroy).toHaveBeenCalledOnce();
  });

  it("keeps the original activation error while rollback retries candidate cleanup", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const candidate = fakeRuntime("candidate");
    const slot = new PetRuntimeSlot(root, oldRuntime);
    vi.mocked(root.replaceChildren).mockImplementationOnce(() => {
      throw new Error("mount failed");
    });
    vi.mocked(candidate.host.destroy).mockImplementationOnce(() => {
      throw new Error("candidate cleanup failed");
    });
    const swap = slot.prepare(candidate);

    expect(() => swap.activate()).toThrow("mount failed");
    expect(candidate.host.destroy).toHaveBeenCalledOnce();

    swap.rollback();
    expect(candidate.host.destroy).toHaveBeenCalledTimes(2);
    expect(slot.activePetId).toBe("old");
  });

  it("re-attaches a changed surface only when its runtime is still active", () => {
    const root = fakeRoot();
    const oldRuntime = fakeRuntime("old");
    const oldFallbackSurface = { dataset: { petId: "old-fallback" } } as unknown as HTMLCanvasElement;
    vi.spyOn(oldRuntime, "getSurface").mockReturnValue(oldFallbackSurface);
    const slot = new PetRuntimeSlot(root, oldRuntime);
    const refreshActiveSurface = (slot as unknown as {
      refreshActiveSurface(runtime: MountedPetRuntime, refresh?: () => void): boolean;
    }).refreshActiveSurface;
    const refreshHitRegion = vi.fn();

    expect(refreshActiveSurface.call(slot, oldRuntime, refreshHitRegion)).toBe(true);
    expect(root.replaceChildren).toHaveBeenLastCalledWith(oldFallbackSurface);
    expect(refreshHitRegion).toHaveBeenCalledOnce();

    const candidate = fakeRuntime("candidate");
    const swap = slot.prepare(candidate);
    swap.activate();
    expect(refreshActiveSurface.call(slot, oldRuntime, refreshHitRegion)).toBe(false);
    expect(root.replaceChildren).toHaveBeenLastCalledWith(candidate.getSurface());
    expect(refreshHitRegion).toHaveBeenCalledOnce();
  });
});
