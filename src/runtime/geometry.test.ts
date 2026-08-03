import { describe, expect, it } from "vitest";
import { computeContainRect } from "./geometry";

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
