import { describe, expect, it, vi } from "vitest";
import { isLive2DRenderAsset, type PetRenderAsset, type PetRenderer } from "./pet-renderer";
import { validMotionProfile } from "./animated-image-test-fixtures";

describe("PetRenderer contract", () => {
  it("distinguishes Live2D assets from static fallbacks", () => {
    const staticAsset: PetRenderAsset = { kind: "static-png", imageUrl: "pet.png" };
    const animatedImageAsset: PetRenderAsset = {
      kind: "animated-image",
      imageUrl: "body.png",
      motionProfile: validMotionProfile(),
    };
    const live2dAsset: PetRenderAsset = {
      kind: "live2d",
      modelUrl: "pet.model3.json",
      previewUrl: "preview.png",
      semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
      dispose: vi.fn(),
    };

    expect(isLive2DRenderAsset(staticAsset)).toBe(false);
    expect(isLive2DRenderAsset(animatedImageAsset)).toBe(false);
    expect(isLive2DRenderAsset(live2dAsset)).toBe(true);
  });

  it("keeps renderer controls backend independent", () => {
    const renderer: PetRenderer = {
      load: vi.fn(async () => {}),
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

    expect(renderer.playMotion("idle")).toEqual({ cancel: expect.any(Function) });
  });
});
