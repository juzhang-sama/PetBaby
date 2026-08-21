import { describe, expect, it, vi } from "vitest";
import { CatBlinkOverlay } from "./cat-blink-overlay";

function fakeImage(): HTMLImageElement {
  return {
    style: {},
    dataset: {},
    remove: vi.fn(),
  } as unknown as HTMLImageElement;
}

describe("CatBlinkOverlay", () => {
  it("mounts one clipped layer per eye and maps eye-open values to opacity", () => {
    const images = [fakeImage(), fakeImage()];
    const parent = { append: vi.fn() } as unknown as HTMLElement;
    const canvas = { parentElement: parent } as unknown as HTMLCanvasElement;
    let index = 0;
    const overlay = new CatBlinkOverlay(canvas, "blob:eyelids", {
      createImage: () => images[index++]!,
    });

    overlay.setVisible(true);
    overlay.setEyesOpen(0.15, 0.8);

    expect(parent.append).toHaveBeenCalledWith(images[0]);
    expect(parent.append).toHaveBeenCalledWith(images[1]);
    expect(images[0]!.dataset.eye).toBe("left");
    expect(images[1]!.dataset.eye).toBe("right");
    expect(images[0]!.style.opacity).toBe("1");
    expect(images[1]!.style.opacity).toBe("1");
    expect(images[0]!.style.transform).toBe("scaleY(0.85)");
    expect(Number(images[1]!.style.transform.match(/[\d.]+/)?.[0])).toBeCloseTo(0.2);
    expect(images[0]!.style.visibility).toBe("visible");
    expect(images[1]!.style.visibility).toBe("visible");

    overlay.destroy();
    expect(images[0]!.remove).toHaveBeenCalledOnce();
    expect(images[1]!.remove).toHaveBeenCalledOnce();
  });
});
