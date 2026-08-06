import { describe, expect, it } from "vitest";
import {
  computeAnchorPosition,
  computeLayerLayout,
  computeSubjectLayerLayout,
  toPixiMatrix,
} from "./layered-sprite";

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

describe("computeSubjectLayerLayout", () => {
  it("scales so the opaque subject fills the viewport", () => {
    const source = { width: 1254, height: 1254 };
    const subject = { x: 262, y: 109, width: 879, height: 1058 };
    const viewport = { width: 420, height: 520 };
    const layout = computeSubjectLayerLayout(source, subject, viewport);
    const fit = computeLayerLayout(
      { width: subject.width, height: subject.height },
      viewport,
    );
    expect(layout.scale).toBeCloseTo(fit.scale, 5);
    // the subject's top-left corner must land exactly where a subject-sized
    // asset would be placed
    expect(layout.x + subject.x * layout.scale).toBeCloseTo(fit.x, 3);
    expect(layout.y + subject.y * layout.scale).toBeCloseTo(fit.y, 3);
  });

  it("keeps the subject bottom-aligned in the viewport", () => {
    const source = { width: 1254, height: 1254 };
    const subject = { x: 262, y: 109, width: 879, height: 1058 };
    const viewport = { width: 420, height: 520 };
    const layout = computeSubjectLayerLayout(source, subject, viewport);
    const subjectBottom = layout.y + (subject.y + subject.height) * layout.scale;
    expect(subjectBottom).toBeLessThanOrEqual(520 + 0.001);
    expect(subjectBottom).toBeGreaterThan(500);
  });

  it("matches plain layout when the subject covers the whole source", () => {
    const source = { width: 512, height: 512 };
    const subject = { x: 0, y: 0, width: 512, height: 512 };
    const viewport = { width: 420, height: 520 };
    const layout = computeSubjectLayerLayout(source, subject, viewport);
    const plain = computeLayerLayout(source, viewport);
    expect(layout.scale).toBeCloseTo(plain.scale, 5);
    expect(layout.x).toBeCloseTo(plain.x, 5);
    expect(layout.y).toBeCloseTo(plain.y, 5);
  });
});

describe("computeAnchorPosition", () => {
  it("keeps the viewport bottom-center fixed when user scale is applied", () => {
    const viewport = { width: 420, height: 520 };
    const anchor = computeAnchorPosition(viewport, 0.5);
    // children are laid out in viewport coordinates and the container applies
    // `position + local * scale`; the offset keeps the bottom-center fixed
    expect(anchor.x).toBeCloseTo(105, 5);
    expect(anchor.y).toBeCloseTo(260, 5);
    // a child laid out at the viewport bottom-center must render there
    const onScreenX = anchor.x + (420 / 2) * 0.5;
    const onScreenY = anchor.y + 520 * 0.5;
    expect(onScreenX).toBeCloseTo(210, 5);
    expect(onScreenY).toBeCloseTo(520, 5);
  });

  it("returns the origin for scale 1", () => {
    const anchor = computeAnchorPosition({ width: 420, height: 520 }, 1);
    expect(anchor.x).toBe(0);
    expect(anchor.y).toBe(0);
  });
});

describe("toPixiMatrix", () => {
  it("maps points exactly like the affine transform", () => {
    const matrix = toPixiMatrix({ a: 2, b: 0, c: 0, d: 3, tx: 10, ty: 20 });
    const out = matrix.apply({ x: 1, y: 2 });
    expect(out.x).toBe(12);
    expect(out.y).toBe(26);
  });
});
