import { describe, expect, it, vi } from "vitest";
import { DEFAULT_PET_CALIBRATION, type PetCalibrationV1 } from "./pet-calibration";
import type { PetRenderAsset, PetRenderer } from "./pet-renderer";
import { PetRendererHost } from "./pet-renderer-host";

function calibration(overrides: Partial<PetCalibrationV1> = {}): PetCalibrationV1 {
  return { ...DEFAULT_PET_CALIBRATION, ...overrides };
}

function fakeRenderer(): PetRenderer {
  return {
    load: vi.fn(async () => undefined),
    resize: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    setExpression: vi.fn(),
    setLookTarget: vi.fn(),
    setLipSync: vi.fn(),
    setCalibration: vi.fn(),
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

  it("replays the latest calibration when replacing the backend", async () => {
    const first = fakeRenderer();
    const second = fakeRenderer();
    const host = new PetRendererHost(first);
    const value = calibration({ breathAmplitudePercent: 4, blinkIntervalScale: 1.5 });

    host.setCalibration(value);
    await host.replace(second, { kind: "static-png", imageUrl: "preview.png" });

    expect(first.setCalibration).toHaveBeenCalledWith(value);
    expect(second.setCalibration).toHaveBeenCalledWith(value);
  });

  it("rejects calibration calls after destruction without reaching the backend", () => {
    const renderer = fakeRenderer();
    const host = new PetRendererHost(renderer);
    host.destroy();

    expect(() => host.setCalibration(DEFAULT_PET_CALIBRATION)).toThrow("PetRendererHost has been destroyed");
    expect(renderer.setCalibration).not.toHaveBeenCalled();
  });

  it("keeps a value snapshot when a renderer mutates its calibration argument", async () => {
    const mutating = fakeRenderer();
    const replacement = fakeRenderer();
    vi.mocked(mutating.setCalibration!).mockImplementation((value) => {
      value.breathAmplitudePercent = 5;
      value.blinkIntervalScale = 2;
      value.feedbackStrength = 1;
    });
    const host = new PetRendererHost(mutating);
    const value = calibration({ breathAmplitudePercent: 1, blinkIntervalScale: 0.75, feedbackStrength: 0.25 });

    host.setCalibration(value);
    await host.replace(replacement, { kind: "static-png", imageUrl: "preview.png" });

    expect(value).toEqual(calibration({
      breathAmplitudePercent: 1,
      blinkIntervalScale: 0.75,
      feedbackStrength: 0.25,
    }));
    expect(replacement.setCalibration).toHaveBeenCalledWith(value);
  });

  it("rejects an invalid calibration without forwarding or replacing its cached snapshot", async () => {
    const first = fakeRenderer();
    const replacement = fakeRenderer();
    const host = new PetRendererHost(first);
    const value = calibration({ blinkIntervalScale: 1.5 });
    host.setCalibration(value);

    expect(() => host.setCalibration({
      ...value,
      blinkIntervalScale: Number.NaN,
    })).toThrow(/calibration/i);
    await host.replace(replacement, { kind: "static-png", imageUrl: "preview.png" });

    expect(first.setCalibration).toHaveBeenCalledOnce();
    expect(replacement.setCalibration).toHaveBeenCalledWith(value);
  });
});
