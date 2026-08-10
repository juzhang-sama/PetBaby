import { describe, expect, it } from "vitest";
import { assertVisiblePixels } from "./render-surface-probe";

describe("assertVisiblePixels", () => {
  it("rejects an all-transparent candidate surface", () => {
    const pixels = new Uint8ClampedArray(4 * 4 * 4);

    expect(() => assertVisiblePixels(pixels)).toThrow("blank-frame");
  });

  it("accepts a surface containing one visible pixel", () => {
    const pixels = new Uint8ClampedArray(4 * 4 * 4);
    pixels[3] = 255;

    expect(() => assertVisiblePixels(pixels)).not.toThrow();
  });
});
