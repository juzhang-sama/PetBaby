import { describe, expect, it, vi } from "vitest";
import { EffectOverlay } from "./effect-overlay";

function fakeElement() {
  return {
    className: "",
    dataset: {} as Record<string, string>,
    hidden: false,
    textContent: "",
    style: { setProperty: vi.fn() },
    append: vi.fn(),
    replaceChildren: vi.fn(),
    remove: vi.fn(),
  } as unknown as HTMLElement;
}

describe("EffectOverlay", () => {
  it("mounts a non-interactive layer and replaces the active effect", () => {
    const root = { append: vi.fn() } as unknown as HTMLElement;
    const elements: HTMLElement[] = [];
    const createElement = vi.fn(() => {
      const element = fakeElement();
      elements.push(element);
      return element;
    });
    const overlay = new EffectOverlay(root, {
      createElement,
      setTimer: vi.fn(() => 1),
      clearTimer: vi.fn(),
    });

    overlay.play("hearts");
    overlay.play("sparkles");

    expect(root.append).toHaveBeenCalledWith(elements[0]);
    expect(elements[0]?.dataset.effect).toBe("sparkles");
    expect(elements[0]?.replaceChildren).toHaveBeenLastCalledWith(...elements.slice(8, 15));
  });

  it("clears particles after the effect duration and destroys idempotently", () => {
    const root = { append: vi.fn() } as unknown as HTMLElement;
    const layer = fakeElement();
    const particles = Array.from({ length: 7 }, fakeElement);
    let clearEffect: (() => void) | undefined;
    const overlay = new EffectOverlay(root, {
      createElement: vi.fn()
        .mockReturnValueOnce(layer)
        .mockImplementation(() => particles.shift() ?? fakeElement()),
      setTimer: vi.fn((callback) => { clearEffect = callback; return 1; }),
      clearTimer: vi.fn(),
    });

    overlay.play("landing");
    clearEffect?.();
    overlay.destroy();
    overlay.destroy();

    expect(layer.hidden).toBe(true);
    expect(layer.remove).toHaveBeenCalledOnce();
  });
});
