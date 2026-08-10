import { describe, expect, it, vi } from "vitest";
import type { PetRenderAsset, PetRenderer } from "./pet-renderer";
import { PetRendererHost } from "./pet-renderer-host";

function fakeRenderer(): PetRenderer {
  return {
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
}

describe("PetRendererHost", () => {
  it("preserves viewport and visibility when replacing the backend", async () => {
    const first = fakeRenderer();
    const second = fakeRenderer();
    const host = new PetRendererHost(first);
    const asset: PetRenderAsset = { kind: "static-png", imageUrl: "preview.png" };
    host.resize({ width: 420, height: 520, dpr: 2 });
    host.setVisibility(true);

    await host.replace(second, asset);

    expect(second.load).toHaveBeenCalledWith(asset);
    expect(second.resize).toHaveBeenCalledWith({ width: 420, height: 520, dpr: 2 });
    expect(second.setVisibility).toHaveBeenCalledWith(true);
    expect(first.destroy).toHaveBeenCalledOnce();
    host.update(16);
    expect(second.update).toHaveBeenCalledWith(16);
  });

  it("keeps the current backend when replacement loading fails", async () => {
    const first = fakeRenderer();
    const failed = fakeRenderer();
    vi.mocked(failed.load).mockRejectedValueOnce(new Error("bad preview"));
    const host = new PetRendererHost(first);

    await expect(host.replace(failed, { kind: "static-png", imageUrl: "bad.png" })).rejects.toThrow("bad preview");

    expect(failed.destroy).toHaveBeenCalledOnce();
    expect(first.destroy).not.toHaveBeenCalled();
    host.update(16);
    expect(first.update).toHaveBeenCalledWith(16);
  });
});
