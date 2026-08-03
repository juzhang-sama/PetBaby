import { describe, expect, it } from "vitest";
import { alphaToRegionSpans } from "./hit-mask";

function rgba(alphas: number[]): Uint8ClampedArray {
  return Uint8ClampedArray.from(alphas.flatMap((alpha) => [0, 0, 0, alpha]));
}

describe("alphaToRegionSpans", () => {
  it("returns no spans for a transparent image", () => {
    expect(alphaToRegionSpans(rgba([0, 0, 0, 0]), 2, 2)).toEqual([]);
  });

  it("merges adjacent opaque pixels into row spans", () => {
    const spans = alphaToRegionSpans(rgba([
      0, 255, 255, 0,
      255, 255, 0, 0,
    ]), 4, 2, { alphaThreshold: 32, rowStep: 1 });

    expect(spans).toEqual([
      { left: 1, top: 0, right: 3, bottom: 1 },
      { left: 0, top: 1, right: 2, bottom: 2 },
    ]);
  });

  it("expands sampled rows to cover the skipped rows", () => {
    const spans = alphaToRegionSpans(rgba([
      255, 0,
      0, 0,
      255, 255,
      0, 0,
    ]), 2, 4, { rowStep: 2, alphaThreshold: 32 });

    expect(spans).toEqual([
      { left: 0, top: 0, right: 1, bottom: 2 },
      { left: 0, top: 2, right: 2, bottom: 4 },
    ]);
  });
});
