import { describe, expect, it, vi } from "vitest";
import { StaticPngRenderer } from "./static-png-renderer";

function createHarness() {
  const context = {
    clearRect: vi.fn(),
    drawImage: vi.fn(),
    setTransform: vi.fn(),
  };
  const canvas = {
    width: 0,
    height: 0,
    style: {} as CSSStyleDeclaration,
    getContext: vi.fn(() => context),
    remove: vi.fn(),
  } as unknown as HTMLCanvasElement;
  const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
  const image = { width: 200, height: 400 } as unknown as CanvasImageSource & { width: number; height: number };
  const loadImage = vi.fn(async () => image);
  const renderer = new StaticPngRenderer(root, { createCanvas: () => canvas, loadImage });
  return { canvas, context, image, loadImage, renderer, root };
}

describe("StaticPngRenderer", () => {
  it("destroy is idempotent and clears visibility", async () => {
    const { renderer } = createHarness();
    await renderer.load({ kind: "static-png", imageUrl: "pet.png" });
    renderer.setVisibility(true);
    renderer.destroy();
    renderer.destroy();
    expect(renderer.state()).toEqual({ loaded: false, visible: false, destroyed: true });
  });

  it("draws a contained image at device pixel resolution", async () => {
    const { canvas, context, image, renderer, root } = createHarness();
    renderer.resize({ width: 400, height: 400, dpr: 2 });
    await renderer.load({ kind: "static-png", imageUrl: "pet.png" });

    expect(root.replaceChildren).toHaveBeenCalledWith(canvas);
    expect(canvas.width).toBe(800);
    expect(canvas.height).toBe(800);
    expect(canvas.style.width).toBe("400px");
    expect(canvas.style.height).toBe("400px");
    expect(context.setTransform).toHaveBeenCalledWith(2, 0, 0, 2, 0, 0);
    expect(context.drawImage).toHaveBeenLastCalledWith(image, 100, 0, 200, 400);
  });

  it("hit tests the visible body bounds", async () => {
    const { renderer } = createHarness();
    renderer.resize({ width: 400, height: 400, dpr: 1 });
    await renderer.load({ kind: "static-png", imageUrl: "pet.png" });
    renderer.setVisibility(true);

    expect(renderer.hitTest({ x: 150, y: 10 })).toBe("body");
    expect(renderer.hitTest({ x: 20, y: 10 })).toBeNull();
    expect(renderer.hitTest({ x: 300, y: 10 })).toBeNull();
    expect(renderer.hitTest({ x: 150, y: 400 })).toBeNull();
    renderer.setVisibility(false);
    expect(renderer.hitTest({ x: 150, y: 10 })).toBeNull();
  });

  it("rejects non-static assets", async () => {
    const { renderer } = createHarness();
    await expect(renderer.load({
      kind: "live2d",
      modelUrl: "pet.model3.json",
      previewUrl: "preview.png",
      semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
      dispose: vi.fn(),
    })).rejects.toThrow("StaticPngRenderer only accepts static-png assets");
  });

  it("ignores an older image that resolves after a newer load", async () => {
    const { canvas, context, renderer } = createHarness();
    const oldImage = { width: 100, height: 100 } as unknown as CanvasImageSource & { width: number; height: number };
    const newImage = { width: 200, height: 400 } as unknown as CanvasImageSource & { width: number; height: number };
    let resolveOld: ((image: typeof oldImage) => void) | undefined;
    let resolveNew: ((image: typeof newImage) => void) | undefined;
    const loadImage = vi.fn((url: string) => new Promise<typeof oldImage>((resolve) => {
      if (url === "old.png") resolveOld = resolve;
      else resolveNew = resolve;
    }));
    const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
    const concurrentRenderer = new StaticPngRenderer(root, { createCanvas: () => canvas, loadImage });
    concurrentRenderer.resize({ width: 400, height: 400, dpr: 1 });

    const oldLoad = concurrentRenderer.load({ kind: "static-png", imageUrl: "old.png" });
    const newLoad = concurrentRenderer.load({ kind: "static-png", imageUrl: "new.png" });
    resolveNew?.(newImage);
    await newLoad;
    resolveOld?.(oldImage);
    await oldLoad;

    expect(context.drawImage).toHaveBeenLastCalledWith(newImage, 100, 0, 200, 400);
  });

  it("lets an in-flight load finish harmlessly after destroy", async () => {
    const { canvas, renderer } = createHarness();
    const image = { width: 200, height: 400 } as unknown as CanvasImageSource & { width: number; height: number };
    let resolveImage: ((value: typeof image) => void) | undefined;
    const loadImage = vi.fn(() => new Promise<typeof image>((resolve) => { resolveImage = resolve; }));
    const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
    const concurrentRenderer = new StaticPngRenderer(root, { createCanvas: () => canvas, loadImage });

    const loading = concurrentRenderer.load({ kind: "static-png", imageUrl: "pet.png" });
    concurrentRenderer.destroy();
    resolveImage?.(image);

    await expect(loading).resolves.toBeUndefined();
    expect(concurrentRenderer.state()).toEqual({ loaded: false, visible: false, destroyed: true });
    expect(root.replaceChildren).not.toHaveBeenCalled();
  });
});
