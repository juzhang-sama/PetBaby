import { describe, expect, it } from "vitest";
import { computeLayerLayout } from "./layered-sprite";

describe("computeLayerLayout", () => {
  it("scales all layers uniformly to the viewport", () => {
    const layout = computeLayerLayout(
      { width: 512, height: 512 },
      { width: 420, height: 520 },
    );
    expect(layout.scale).toBeCloseTo(420 / 512, 5);
    expect(layout.width).toBeCloseTo(420, 5);
  });

  it("keeps the pet anchored near the bottom of the viewport", () => {
    const layout = computeLayerLayout(
      { width: 512, height: 512 },
      { width: 420, height: 520 },
    );
    expect(layout.y).toBeGreaterThan(0);
    expect(layout.y + layout.height).toBeLessThanOrEqual(520 + 0.001);
  });

  it("centers the asset horizontally", () => {
    const layout = computeLayerLayout(
      { width: 512, height: 512 },
      { width: 420, height: 520 },
    );
    expect(layout.x).toBe(0);
  });
});
