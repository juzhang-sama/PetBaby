import { describe, expect, it } from "vitest";
import { clampRectToWorkArea, computeContainRect } from "./geometry";

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

it("clamps a bottom-off-screen window fully into the work area", () => {
  const rect = { x: 1166, y: 700, width: 420, height: 520 };
  const clamped = clampRectToWorkArea(rect, { x: 0, y: 0, width: 1920, height: 1080 }, Math.max(rect.width, rect.height));
  expect(clamped.y + rect.height).toBeLessThanOrEqual(1080);
  expect(clamped.x).toBeGreaterThanOrEqual(0);
});
