import { describe, expect, it, vi } from "vitest";
import { validMotionProfile } from "./animated-image-test-fixtures";
import { LIFE_V1, planBreathSlices, type BreathSlice } from "./animated-image-motion";
import { AnimatedImageRenderer } from "./animated-image-renderer";
import { DEFAULT_PET_CALIBRATION } from "./pet-calibration";

function rendererHarness() {
  const contexts = Array.from({ length: 3 }, () => ({
    clearRect: vi.fn(),
    drawImage: vi.fn(),
    setTransform: vi.fn(),
    save: vi.fn(),
    restore: vi.fn(),
    translate: vi.fn(),
    rotate: vi.fn(),
    scale: vi.fn(),
  }));
  const canvases = contexts.map((context) => ({
    width: 0,
    height: 0,
    style: {} as CSSStyleDeclaration,
    getContext: vi.fn(() => context),
    remove: vi.fn(),
  } as unknown as HTMLCanvasElement));
  let canvasIndex = 0;
  let lastPlans: BreathSlice[] = [];
  const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
  const loadedImage = {
    width: 1000,
    height: 1000,
  } as CanvasImageSource & { width: number; height: number };
  const loadImage = vi.fn(async () => loadedImage);
  const renderer = new AnimatedImageRenderer(root, {
    createCanvas: () => canvases[canvasIndex++]!,
    loadImage,
    planBreathSlices: (...args) => (lastPlans = planBreathSlices(...args)),
  });
  renderer.setVisibility(true);
  return {
    renderer,
    root,
    loadImage,
    context: contexts[0]!,
    hitContext: contexts[1]!,
    composeContext: contexts[2]!,
    displayCanvas: canvases[0]!,
    hitCanvas: canvases[1]!,
    composeCanvas: canvases[2]!,
    loadedImage,
    faceSafeY: 500,
    localPlans: () => lastPlans,
  };
}

describe("AnimatedImageRenderer", () => {
  it("does not expose the frozen whole-image blink effect", () => {
    const { renderer } = rendererHarness();
    expect("setBlink" in renderer).toBe(false);
  });

  it("precomposes only raster-safe breath slices before swaying one texture", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    test.context.drawImage.mockClear();
    test.composeContext.drawImage.mockClear();

    test.renderer.playMotion("idle");
    test.renderer.update(1300);

    expect(test.composeContext.drawImage).toHaveBeenCalledTimes(test.localPlans().length);
    expect(test.composeContext.drawImage.mock.calls.every((call) =>
      call.length === 9 && call[0] === test.loadedImage
    )).toBe(true);
    expect(test.context.drawImage).toHaveBeenCalledOnce();
    expect(test.context.drawImage).toHaveBeenCalledWith(
      test.composeCanvas,
      0,
      0,
      800,
      1000,
      0,
      0,
      400,
      500,
    );
    expect(test.context.rotate).toHaveBeenLastCalledWith(0.7 * Math.PI / 180);
  });

  it("keeps the offscreen compose canvas out of the rendered DOM", async () => {
    const test = rendererHarness();
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });

    expect(test.root.replaceChildren).toHaveBeenCalledWith(test.displayCanvas, test.hitCanvas);
    expect(test.root.replaceChildren).not.toHaveBeenCalledWith(
      test.displayCanvas,
      test.hitCanvas,
      test.composeCanvas,
    );
  });

  it("renders idle updates and keeps the face slices locally unchanged", async () => {
    const test = rendererHarness();
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    test.renderer.resize({ width: 420, height: 520, dpr: 2 });
    test.renderer.playMotion("idle", { loop: true });
    test.renderer.update(700);

    expect(test.context.drawImage).toHaveBeenCalled();
    expect(test.localPlans()
      .filter((slice) => slice.sourceY + slice.sourceHeight <= test.faceSafeY)
      .every((slice) =>
        slice.destX === slice.sourceX
        && slice.destY === slice.sourceY
        && slice.destWidth === slice.sourceWidth
        && slice.destHeight === slice.sourceHeight
      )).toBe(true);
  });

  it("composes sway only as a final pivot rotation and horizontal translation", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 400, dpr: 1 });
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    test.renderer.playMotion("idle");
    test.renderer.update(1300);

    expect(test.context.rotate).toHaveBeenLastCalledWith(0.7 * Math.PI / 180);
    expect(test.context.translate).toHaveBeenCalledWith(201.8, 288);
    expect(test.context.translate).toHaveBeenCalledWith(-200, -288);
  });

  it("applies calibrated breathing without a whole-canvas blink transform", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 1 });
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    const hitDraws = test.hitContext.drawImage.mock.calls.length;
    test.renderer.setCalibration({ ...DEFAULT_PET_CALIBRATION, breathAmplitudePercent: 4 });
    test.context.translate.mockClear();
    test.context.scale.mockClear();
    test.renderer.playMotion("idle");

    test.renderer.update(1400);

    expect(test.context.scale).not.toHaveBeenCalledWith(1, 0.97);
    expect(test.hitContext.drawImage).toHaveBeenCalledTimes(hitDraws);
    const deformed = test.localPlans().find((slice) => slice.destWidth !== slice.sourceWidth)!;
    const baseline = planBreathSlices(validMotionProfile(), 1000, 1000, 1, 24, 2)
      .find((slice) => slice.sourceX === deformed.sourceX && slice.sourceY === deformed.sourceY)!;
    expect(deformed.destWidth - deformed.sourceWidth).toBeCloseTo(
      2 * (baseline.destWidth - baseline.sourceWidth),
    );
  });

  it("does not render or advance calibrated motion while hidden", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 1 });
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    test.renderer.setCalibration({ ...DEFAULT_PET_CALIBRATION, breathAmplitudePercent: 4 });
    test.renderer.playMotion("idle");
    test.renderer.setVisibility(false);
    test.context.drawImage.mockClear();

    test.renderer.update(60_000);

    expect(test.context.drawImage).not.toHaveBeenCalled();
    test.renderer.setVisibility(true);
    test.renderer.update(1_400);
    const deformed = test.localPlans().find((slice) => slice.destWidth !== slice.sourceWidth)!;
    const baseline = planBreathSlices(validMotionProfile(), 1000, 1000, 1, 24, 2)
      .find((slice) => slice.sourceX === deformed.sourceX && slice.sourceY === deformed.sourceY)!;
    expect(deformed.destWidth - deformed.sourceWidth).toBeCloseTo(
      2 * (baseline.destWidth - baseline.sourceWidth),
    );
  });

  it("draws the correlated sway trajectory into a stable load-or-resize hit envelope", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 420, height: 520, dpr: 2 });
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    expect(test.renderer.getHitSurface()).toBe(test.hitCanvas);
    const pivotX = 210;
    const hitTransforms = test.hitContext.rotate.mock.calls.map((call, index) => ({
      swayX: test.hitContext.translate.mock.calls[index * 2]![0] - pivotX,
      swayRadians: call[0],
    }));
    for (const scalar of [-1, 0.5, 1]) {
      expect(hitTransforms.some((transform) =>
        Math.abs(transform.swayX - 420 * LIFE_V1.swayXRatio * scalar) < 1e-12
        && Math.abs(transform.swayRadians - LIFE_V1.swayRadians * scalar) < 1e-12
      )).toBe(true);
    }
    const hitDrawsAfterLoad = test.hitContext.drawImage.mock.calls.length;
    expect(hitDrawsAfterLoad).toBeGreaterThan(5);

    test.renderer.playMotion("idle");
    test.renderer.update(100);
    test.renderer.update(100);
    expect(test.hitContext.drawImage).toHaveBeenCalledTimes(hitDrawsAfterLoad);

    test.renderer.resize({ width: 300, height: 450, dpr: 1 });
    expect(test.hitContext.drawImage).toHaveBeenCalledTimes(hitDrawsAfterLoad * 2);
  });

  it("consumes a subpixel hit-envelope plan at large viewport scale", async () => {
    const test = rendererHarness();
    const viewport = { width: 3840, height: 2160, dpr: 3 };
    test.renderer.resize(viewport);
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    const bounds = { x: 840, y: 0, width: 2160, height: 2160 };
    const pivot = { x: 1920, y: 1555.2 };
    const rMax = Math.max(
      Math.hypot(bounds.x - pivot.x, bounds.y - pivot.y),
      Math.hypot(bounds.x + bounds.width - pivot.x, bounds.y - pivot.y),
      Math.hypot(bounds.x - pivot.x, bounds.y + bounds.height - pivot.y),
      Math.hypot(bounds.x + bounds.width - pivot.x, bounds.y + bounds.height - pivot.y),
    );
    const transforms = test.hitContext.rotate.mock.calls.map((call, index) => ({
      swayXRatio: (test.hitContext.translate.mock.calls[index * 2]![0] - pivot.x) / viewport.width,
      swayRadians: call[0],
    }));
    const physicalSteps = transforms.slice(1).map((transform, index) => {
      const previous = transforms[index]!;
      return viewport.dpr * (
        viewport.width * Math.abs(transform.swayXRatio - previous.swayXRatio)
        + rMax * Math.abs(transform.swayRadians - previous.swayRadians)
      );
    });

    expect(transforms.length).toBeGreaterThan(33);
    expect(Math.max(...physicalSteps)).toBeLessThanOrEqual(1 + 1e-9);
    const hitDrawsAfterLoad = test.hitContext.drawImage.mock.calls.length;
    test.renderer.playMotion("idle");
    test.renderer.update(100);
    expect(test.hitContext.drawImage).toHaveBeenCalledTimes(hitDrawsAfterLoad);
  });

  it("uses anonymous browser image loading through the shared loader", async () => {
    const test = rendererHarness();
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    expect(test.loadImage).toHaveBeenCalledWith("pet.png");
  });

  it("renders full-image actions and resumes idle motion", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 400, dpr: 1 });
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    const drawsBefore = test.context.drawImage.mock.calls.length;

    test.renderer.playMotion("react-happy");
    test.renderer.update(1300);
    const drawsAfterAction = test.context.drawImage.mock.calls.length;
    expect(drawsAfterAction).toBeGreaterThan(drawsBefore);

    test.renderer.playMotion("idle");
    test.renderer.update(1300);
    expect(test.context.drawImage.mock.calls.length).toBeGreaterThan(drawsAfterAction);
  });

  it("hides both canvases when visibility is disabled", () => {
    const test = rendererHarness();
    test.renderer.setVisibility(true);
    test.renderer.setVisibility(false);
    expect(test.displayCanvas.style.visibility).toBe("hidden");
    expect(test.hitCanvas.style.visibility).toBe("hidden");
  });

  it("destroys display and hit canvases exactly once", async () => {
    const test = rendererHarness();
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    test.renderer.destroy();
    test.renderer.destroy();
    expect(test.context.clearRect).toHaveBeenCalledOnce();
    expect(test.hitContext.clearRect).toHaveBeenCalledOnce();
    expect(test.composeContext.clearRect).toHaveBeenCalledOnce();
    expect(test.displayCanvas.remove).toHaveBeenCalledOnce();
    expect(test.hitCanvas.remove).toHaveBeenCalledOnce();
    expect(test.composeCanvas.remove).toHaveBeenCalledOnce();
  });
});
