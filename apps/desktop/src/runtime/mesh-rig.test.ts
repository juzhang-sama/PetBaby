import { describe, expect, it } from "vitest";
import {
  buildGrid,
  deformGrid,
  heuristicFeatures,
  type FeatureRects,
  type GridData,
} from "./mesh-rig";

const SUBJECT = { x: 100, y: 80, width: 400, height: 500 };
const SIZE = { width: 600, height: 700 };

function makeGrid(): GridData {
  return buildGrid(SIZE.width, SIZE.height, 8, 8);
}

describe("buildGrid", () => {
  it("covers the full texture with a triangle grid", () => {
    const grid = makeGrid();
    expect(grid.positions).toHaveLength(8 * 8 * 2);
    expect(grid.uvs).toHaveLength(8 * 8 * 2);
    expect(grid.indices).toHaveLength((8 - 1) * (8 - 1) * 6);
    expect(grid.positions[0]).toBe(0);
    expect(grid.positions[1]).toBe(0);
    expect(grid.positions.at(-2)).toBe(SIZE.width);
    expect(grid.positions.at(-1)).toBe(SIZE.height);
  });

  it("rejects degenerate sizes", () => {
    expect(() => buildGrid(0, 100, 2, 2)).toThrow(RangeError);
    expect(() => buildGrid(100, 100, 1, 2)).toThrow(RangeError);
  });
});

describe("heuristicFeatures", () => {
  it("places features inside the subject", () => {
    const features = heuristicFeatures(SUBJECT, SIZE.width, SIZE.height);
    expect(features.leftEye.x).toBeGreaterThanOrEqual(SUBJECT.x);
    expect(features.rightEye.x).toBeGreaterThan(features.leftEye.x);
    expect(features.leftEye.y).toBeGreaterThanOrEqual(SUBJECT.y);
    expect(features.leftEar.x).toBeLessThan(features.rightEar.x);
    expect(features.tail.y).toBeGreaterThan(features.leftEye.y);
  });
});

describe("deformGrid", () => {
  const features: FeatureRects = heuristicFeatures(SUBJECT, SIZE.width, SIZE.height);

  it("returns the base positions when nothing is animated", () => {
    const grid = makeGrid();
    const out = deformGrid(grid, features, { blink: 0, earWobble: 0, tailSway: 0 });
    expect(Array.from(out)).toEqual(Array.from(grid.positions));
  });

  it("squashes the eye region toward its center during a blink", () => {
    const grid = makeGrid();
    const out = deformGrid(grid, features, { blink: 1, earWobble: 0, tailSway: 0 });
    let moved = 0;
    for (const eye of [features.leftEye, features.rightEye]) {
      const centerY = eye.y + eye.height / 2;
      for (let row = 0; row < grid.rows; row += 1) {
        for (let col = 0; col < grid.cols; col += 1) {
          const index = row * grid.cols + col;
          const y = grid.positions[index * 2 + 1]!;
          if (y > eye.y - 8 && y < eye.y + eye.height + 8) {
            const outY = out[index * 2 + 1]!;
            expect(Math.abs(outY - centerY)).toBeLessThanOrEqual(Math.abs(y - centerY));
            if (Math.abs(outY - centerY) < Math.abs(y - centerY)) moved += 1;
          }
        }
      }
    }
    expect(moved).toBeGreaterThan(0);
  });

  it("rotates the ears when wobbling", () => {
    const grid = makeGrid();
    const out = deformGrid(grid, features, { blink: 0, earWobble: 1, tailSway: 0 });
    // the top-left vertex sits inside the left ear influence zone
    let moved = 0;
    for (let row = 0; row < grid.rows; row += 1) {
      for (let col = 0; col < grid.cols; col += 1) {
        const index = row * grid.cols + col;
        const x = grid.positions[index * 2]!;
        const y = grid.positions[index * 2 + 1]!;
        if (x >= features.leftEar.x - 8 && x <= features.leftEar.x + features.leftEar.width + 8
          && y >= features.leftEar.y - 8 && y <= features.leftEar.y + features.leftEar.height + 8) {
          if (out[index * 2] !== x || out[index * 2 + 1] !== y) moved += 1;
        }
      }
    }
    expect(moved).toBeGreaterThan(0);
  });

  it("shifts the tail region sideways", () => {
    const grid = makeGrid();
    const out = deformGrid(grid, features, { blink: 0, earWobble: 0, tailSway: Math.PI / 2 });
    let moved = 0;
    for (let row = 0; row < grid.rows; row += 1) {
      for (let col = 0; col < grid.cols; col += 1) {
        const index = row * grid.cols + col;
        const x = grid.positions[index * 2]!;
        const y = grid.positions[index * 2 + 1]!;
        if (x > features.tail.x && x < features.tail.x + features.tail.width
          && y > features.tail.y && y < features.tail.y + features.tail.height) {
          if (out[index * 2] !== x) moved += 1;
        }
      }
    }
    expect(moved).toBeGreaterThan(0);
  });
});
