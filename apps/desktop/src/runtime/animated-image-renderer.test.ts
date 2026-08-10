import { describe, expect, it, vi } from "vitest";
import { validMotionProfile } from "./animated-image-test-fixtures";
import { planBreathSlices, type BreathSlice } from "./animated-image-motion";
import { AnimatedImageRenderer } from "./animated-image-renderer";

function rendererHarness() {
  const contexts = Array.from({ length: 2 }, () => ({
    clearRect: vi.fn(),
    drawImage: vi.fn(),
    setTransform: vi.fn(),
    save: vi.fn(),
    restore: vi.fn(),
    translate: vi.fn(),
    rotate: vi.fn(),
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
  const loadImage = vi.fn(async () => ({
    width: 1000,
    height: 1000,
  } as CanvasImageSource & { width: number; height: number }));
  const renderer = new AnimatedImageRenderer(root, {
    createCanvas: () => canvases[canvasIndex++]!,
    loadImage,
    planBreathSlices: (...args) => (lastPlans = planBreathSlices(...args)),
  });
  return {
    renderer,
    root,
    loadImage,
    context: contexts[0]!,
    hitContext: contexts[1]!,
    displayCanvas: canvases[0]!,
    hitCanvas: canvases[1]!,
    faceSafeY: 500,
    localPlans: () => lastPlans,
  };
}

describe("AnimatedImageRenderer", () => {
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

  it("draws a stable hit envelope on load or resize but not on each update", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 420, height: 520, dpr: 2 });
    await test.renderer.load({
      kind: "animated-image",
      imageUrl: "pet.png",
      motionProfile: validMotionProfile(),
    });
    expect(test.renderer.getHitSurface()).toBe(test.hitCanvas);
    expect(test.hitContext.drawImage).toHaveBeenCalledTimes(5);
    const hitDrawsAfterLoad = test.hitContext.drawImage.mock.calls.length;

    test.renderer.playMotion("idle");
    test.renderer.update(100);
    test.renderer.update(100);
    expect(test.hitContext.drawImage).toHaveBeenCalledTimes(hitDrawsAfterLoad);

    test.renderer.resize({ width: 300, height: 450, dpr: 1 });
    expect(test.hitContext.drawImage).toHaveBeenCalledTimes(hitDrawsAfterLoad + 5);
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

  it("only starts accumulation for idle motion", async () => {
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
    expect(test.context.drawImage).toHaveBeenCalledTimes(drawsBefore);

    test.renderer.playMotion("idle");
    test.renderer.update(1300);
    expect(test.context.drawImage.mock.calls.length).toBeGreaterThan(drawsBefore);
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
    expect(test.displayCanvas.remove).toHaveBeenCalledOnce();
    expect(test.hitCanvas.remove).toHaveBeenCalledOnce();
  });
});
