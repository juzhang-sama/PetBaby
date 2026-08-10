import { describe, expect, it, vi } from "vitest";
import type { PetRendererHost } from "./pet-renderer-host";
import type { PetRendererRuntime } from "./pet-renderer-bootstrap";
import type { PetExpression, PetHitArea, PetMotion, PetMotionHandle, PetRenderAsset, PetRenderer } from "./pet-renderer";
import { PetRuntimeSlot, type MountedPetRuntime } from "./pet-runtime-slot";

function fakeRoot(): HTMLElement {
  return { replaceChildren: vi.fn() } as unknown as HTMLElement;
}

function fakeRuntime(petId: string): MountedPetRuntime {
  const surface = { dataset: { petId } } as unknown as HTMLCanvasElement;
  const host: PetRenderer = {
    load: vi.fn(async (_asset: PetRenderAsset) => undefined),
    resize: vi.fn(),
    playMotion: vi.fn((_motion: PetMotion, _options?: { loop?: boolean; priority?: number }): PetMotionHandle => ({ cancel: vi.fn() })),
    setExpression: vi.fn((_expression: PetExpression, _weight?: number) => undefined),
    setLookTarget: vi.fn((_target: { x: number; y: number } | null) => undefined),
    setLipSync: vi.fn((_value: number) => undefined),
    hitTest: vi.fn((_point: { x: number; y: number }): PetHitArea | null => null),
    setVisibility: vi.fn((_visible: boolean) => undefined),
    update: vi.fn((_deltaMs: number) => undefined),
    destroy: vi.fn(),
  };
  const runtime: PetRendererRuntime = {
    host: host as PetRendererHost,
    getSurface: () => surface,
    kind: () => "static-png",
    recoverToPreview: async () => undefined,
  };
  return { ...runtime, petId };
}

describe("PetRuntimeSlot", () => {
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
});
