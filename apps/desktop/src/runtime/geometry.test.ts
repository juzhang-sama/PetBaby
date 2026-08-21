import { describe, expect, it } from "vitest";
import { clampRectToWorkArea, computeContainRect, displayRectForScale } from "./geometry";

describe("computeContainRect", () => {
  it("centers a portrait asset without cropping", () => {
    expect(computeContainRect(
      { width: 200, height: 400 },
      { width: 400, height: 400 },
    )).toEqual({ x: 100, y: 0, width: 200, height: 400, scale: 1 });
  });

  it("scales down oversized assets", () => {
    expect(computeContainRect(
      { width: 800, height: 400 },
      { width: 400, height: 300 },
    )).toEqual({ x: 0, y: 50, width: 400, height: 200, scale: 0.5 });
  });
});

it("keeps at least 64 pixels visible after a display is removed", () => {
  expect(clampRectToWorkArea(
    { x: 2500, y: 900, width: 420, height: 520 },
    { x: 0, y: 0, width: 1920, height: 1080 },
    64,
  )).toEqual({ x: 1856, y: 900, width: 420, height: 520 });
});

it("leaves an oversized leftover position alone when minimum visibility holds", () => {
  expect(clampRectToWorkArea(
    { x: 702, y: 509, width: 1936, height: 1096 },
    { x: 0, y: 0, width: 1920, height: 1080 },
    64,
  )).toEqual({ x: 702, y: 509, width: 1936, height: 1096 });
});

describe("displayRectForScale", () => {
  const primaryWorkArea = { x: 0, y: 0, width: 1920, height: 1040 };

  it("keeps the bottom center anchored with deterministic odd-width rounding", () => {
    expect(displayRectForScale(
      { x: 1000, y: 400, width: 420, height: 520 },
      0.75,
      primaryWorkArea,
    )).toEqual({ x: 1052, y: 530, width: 315, height: 390 });
  });

  it.each([
    [0.25, 210, 260],
    [0.5, 210, 260],
    [1, 420, 520],
    [1.5, 630, 780],
    [2, 630, 780],
  ])("clamps scale %s to a 50/100/150 percent logical-pixel size", (scale, width, height) => {
    const current = { x: 750, y: 260, width: 420, height: 520 };
    const rect = displayRectForScale(
      current,
      scale,
      primaryWorkArea,
    );

    expect({ width: rect.width, height: rect.height }).toEqual({ width, height });
    expect(rect.x + Math.ceil(rect.width / 2)).toBe(current.x + current.width / 2);
    expect(rect.y + rect.height).toBe(current.y + current.height);
  });

  it("anchors correctly on a negative-coordinate secondary display", () => {
    expect(displayRectForScale(
      { x: -1200, y: 200, width: 420, height: 520 },
      1.5,
      { x: -1920, y: 0, width: 1920, height: 1040 },
    )).toEqual({ x: -1305, y: -60, width: 630, height: 780 });
  });

  it("uses logical pixels without applying a display scale factor", () => {
    const resultAtHighDpi = displayRectForScale(
      { x: 100, y: 100, width: 420, height: 520 },
      1.25,
      primaryWorkArea,
    );

    expect(resultAtHighDpi).toEqual({ x: 47, y: -30, width: 525, height: 650 });
  });

  it("keeps the maximum possible visible strip when the work area is narrower than 64 pixels", () => {
    expect(displayRectForScale(
      { x: 500, y: 500, width: 420, height: 520 },
      0.5,
      { x: -20, y: -10, width: 40, height: 48 },
    )).toEqual({ x: -20, y: -10, width: 210, height: 260 });
  });

  it("clamps a far-offscreen resize while preserving the 64-pixel visibility boundary", () => {
    expect(displayRectForScale(
      { x: 1_000_000, y: 1_000_000, width: 420, height: 520 },
      1,
      primaryWorkArea,
    )).toEqual({ x: 1856, y: 976, width: 420, height: 520 });
  });

  it.each([Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY])(
    "falls back to the base scale for non-finite scale %s",
    (scale) => {
      expect(displayRectForScale(
        { x: 100, y: 100, width: 420, height: 520 },
        scale,
        primaryWorkArea,
      )).toEqual({ x: 100, y: 100, width: 420, height: 520 });
    },
  );

  it("returns finite geometry for non-finite and extreme current rectangles", () => {
    for (const current of [
      { x: Number.NaN, y: Number.POSITIVE_INFINITY, width: 420, height: 520 },
      { x: Number.MAX_VALUE, y: -Number.MAX_VALUE, width: Number.MAX_VALUE, height: Number.MAX_VALUE },
    ]) {
      const rect = displayRectForScale(current, 1, primaryWorkArea);
      expect(Object.values(rect).every(Number.isFinite)).toBe(true);
    }
  });

  it("returns finite geometry when extreme work-area arithmetic would overflow", () => {
    const rect = displayRectForScale(
      { x: Number.NaN, y: Number.POSITIVE_INFINITY, width: 420, height: 520 },
      1,
      {
        x: Number.MAX_VALUE,
        y: Number.MAX_VALUE,
        width: Number.MAX_VALUE,
        height: Number.MAX_VALUE,
      },
    );

    expect(Object.values(rect).every(Number.isFinite)).toBe(true);
  });

  it("does not accumulate more than one pixel of anchor drift across repeated scale changes", () => {
    const initial = { x: 1000, y: 400, width: 420, height: 520 };
    const initialBottomCenter = {
      x: initial.x + initial.width / 2,
      y: initial.y + initial.height,
    };
    let current = initial;

    for (let cycle = 0; cycle < 20; cycle += 1) {
      current = displayRectForScale(current, 0.75, primaryWorkArea);
      current = displayRectForScale(current, 1.25, primaryWorkArea);
      current = displayRectForScale(current, 1, primaryWorkArea);
    }

    expect(Math.abs(current.x + current.width / 2 - initialBottomCenter.x)).toBeLessThanOrEqual(1);
    expect(Math.abs(current.y + current.height - initialBottomCenter.y)).toBeLessThanOrEqual(1);
  });
});
