import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { EffectOverlay } from "./effect-overlay";

const styles = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

function fakeElement() {
  return {
    className: "",
    dataset: {} as Record<string, string>,
    hidden: false,
    textContent: "",
    style: { setProperty: vi.fn(), opacity: "" },
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

  it("consumes calibrated opacity and intensity without changing the effect kind", () => {
    const root = { append: vi.fn() } as unknown as HTMLElement;
    const elements: HTMLElement[] = [];
    const overlay = new EffectOverlay(root, {
      createElement: vi.fn(() => {
        const element = fakeElement();
        elements.push(element);
        return element;
      }),
      setTimer: vi.fn(() => 1),
      clearTimer: vi.fn(),
    });

    overlay.play("hearts", { opacity: 0.25, intensity: 0.25 });

    expect(elements[0]?.dataset.effect).toBe("hearts");
    expect(elements[0]?.style.opacity).toBe("0.25");
    expect(elements[1]?.style.setProperty).toHaveBeenCalledWith("--particle-drift", "-6.75px");
  });

  it.each([
    [0.25, [
      ["--pet-effect-rise-start-y", "3px"],
      ["--pet-effect-rise-end-y", "-13.5px"],
      ["--pet-effect-heart-start-scale", "0.9125"],
      ["--pet-effect-heart-end-scale", "1.02"],
      ["--pet-effect-spark-end-rotation", "22.5deg"],
      ["--pet-effect-spark-start-scale", "0.85"],
      ["--pet-effect-spark-end-scale", "1.0625"],
      ["--pet-effect-land-start-scale-x", "0.8"],
      ["--pet-effect-land-end-scale-x", "1.2"],
    ]],
    [1, [
      ["--pet-effect-rise-start-y", "12px"],
      ["--pet-effect-rise-end-y", "-54px"],
      ["--pet-effect-heart-start-scale", "0.65"],
      ["--pet-effect-heart-end-scale", "1.08"],
      ["--pet-effect-spark-end-rotation", "90deg"],
      ["--pet-effect-spark-start-scale", "0.4"],
      ["--pet-effect-spark-end-scale", "1.25"],
      ["--pet-effect-land-start-scale-x", "0.2"],
      ["--pet-effect-land-end-scale-x", "1.8"],
    ]],
  ] as const)("publishes real animation variables at %s intensity", (intensity, expected) => {
    const root = { append: vi.fn() } as unknown as HTMLElement;
    const elements: HTMLElement[] = [];
    const overlay = new EffectOverlay(root, {
      createElement: vi.fn(() => {
        const element = fakeElement();
        elements.push(element);
        return element;
      }),
      setTimer: vi.fn(() => 1),
      clearTimer: vi.fn(),
    });

    overlay.play("hearts", { opacity: 0.5, intensity });

    for (const [name, value] of expected) {
      expect(elements[0]?.style.setProperty).toHaveBeenCalledWith(name, value);
    }
  });

  it("binds every effect keyframe to the published intensity variables", () => {
    for (const variable of [
      "--pet-effect-rise-start-y",
      "--pet-effect-rise-end-y",
      "--pet-effect-heart-start-scale",
      "--pet-effect-heart-end-scale",
      "--pet-effect-spark-end-rotation",
      "--pet-effect-spark-start-scale",
      "--pet-effect-spark-end-scale",
      "--pet-effect-land-start-scale-x",
      "--pet-effect-land-end-scale-x",
      "--particle-drift",
    ]) expect(styles).toContain(`var(${variable})`);
  });

  it("keeps fallback variables on the overlay so inline calibrated values inherit to particles", () => {
    const overlayRule = Array.from(styles.matchAll(/\.pet-effect-overlay\s*\{[^}]*\}/g))
      .map(([rule]) => rule)
      .find((rule) => rule.includes("overflow")) ?? "";
    const particleRule = styles.match(/\.pet-effect-particle\s*\{[^}]*\}/)?.[0] ?? "";

    expect(overlayRule).toContain("--pet-effect-rise-start-y: 12px");
    expect(particleRule).not.toContain("--pet-effect-rise-start-y");
  });
});
