import { describe, expect, it } from "vitest";
import { validMotionProfile } from "./animated-image-test-fixtures";
import {
  LIFE_V1,
  breathWeight,
  computeMotionFrame,
  planBreathSlices,
  planHitEnvelopeTransforms,
  planRasterSafeBreathSlice,
} from "./animated-image-motion";

describe("animated image motion", () => {
  const defaultHitGeometry = {
    viewportWidth: 420,
    dpr: 2,
    bounds: { x: 0, y: 50, width: 420, height: 420 },
    pivot: { x: 210, y: 352.4 },
  };

  it("adds one physical pixel of overlap across a shared internal edge", () => {
    const destinationScale = 0.4;
    const dpr = 2;
    const left = {
      sourceX: 0,
      sourceY: 0,
      sourceWidth: 500,
      sourceHeight: 1000,
      destX: 0,
      destY: 0,
      destWidth: 500,
      destHeight: 1000,
    };
    const right = {
      ...left,
      sourceX: 500,
      destX: 500,
    };
    const semanticOverlap = (left.destX + left.destWidth - right.destX)
      * destinationScale
      * dpr;
    const safeLeft = planRasterSafeBreathSlice(
      left,
      1000,
      1000,
      destinationScale,
      dpr,
    );
    const safeRight = planRasterSafeBreathSlice(
      right,
      1000,
      1000,
      destinationScale,
      dpr,
    );
    const rasterOverlap = (
      safeLeft.destX + safeLeft.destWidth - safeRight.destX
    ) * destinationScale * dpr;

    expect(semanticOverlap).toBe(0);
    expect(rasterOverlap).toBeGreaterThanOrEqual(1);
    expect(rasterOverlap).toBeCloseTo(1);
  });

  it("never expands the image's external source or destination edges", () => {
    const left = planRasterSafeBreathSlice({
      sourceX: 0,
      sourceY: 0,
      sourceWidth: 500,
      sourceHeight: 1000,
      destX: 0,
      destY: 0,
      destWidth: 500,
      destHeight: 1000,
    }, 1000, 1000, 0.4, 2);
    const right = planRasterSafeBreathSlice({
      sourceX: 500,
      sourceY: 0,
      sourceWidth: 500,
      sourceHeight: 1000,
      destX: 500,
      destY: 0,
      destWidth: 500,
      destHeight: 1000,
    }, 1000, 1000, 0.4, 2);

    expect(left.sourceX).toBe(0);
    expect(left.destX).toBe(0);
    expect(left.sourceY).toBe(0);
    expect(left.destY).toBe(0);
    expect(right.sourceX + right.sourceWidth).toBe(1000);
    expect(right.destX + right.destWidth).toBe(1000);
    expect(right.sourceY + right.sourceHeight).toBe(1000);
    expect(right.destY + right.destHeight).toBe(1000);
  });

  it("maps a half-physical-pixel bleed through deformed destination scales", () => {
    const slice = {
      sourceX: 300,
      sourceY: 400,
      sourceWidth: 400,
      sourceHeight: 100,
      destX: 290,
      destY: 402,
      destWidth: 420,
      destHeight: 104,
    };
    const destinationScale = 0.4;
    const dpr = 2;
    const safe = planRasterSafeBreathSlice(
      slice,
      1000,
      1000,
      destinationScale,
      dpr,
    );
    const destinationBleed = 0.5 / (destinationScale * dpr);
    const sourceBleedX = destinationBleed * slice.sourceWidth / slice.destWidth;
    const sourceBleedY = destinationBleed * slice.sourceHeight / slice.destHeight;

    expect(safe.destX).toBeCloseTo(slice.destX - destinationBleed);
    expect(safe.destY).toBeCloseTo(slice.destY - destinationBleed);
    expect(safe.destWidth).toBeCloseTo(slice.destWidth + 2 * destinationBleed);
    expect(safe.destHeight).toBeCloseTo(slice.destHeight + 2 * destinationBleed);
    expect(safe.sourceX).toBeCloseTo(slice.sourceX - sourceBleedX);
    expect(safe.sourceY).toBeCloseTo(slice.sourceY - sourceBleedY);
    expect(safe.sourceWidth).toBeCloseTo(slice.sourceWidth + 2 * sourceBleedX);
    expect(safe.sourceHeight).toBeCloseTo(slice.sourceHeight + 2 * sourceBleedY);
  });

  it("uses the approved life-v1 periods and amplitudes", () => {
    expect(LIFE_V1).toEqual({
      breathPeriodMs: 2800,
      breathScaleX: 0.028,
      breathShiftY: 0.012,
      swayPeriodMs: 5200,
      swayRadians: 0.7 * Math.PI / 180,
      swayXRatio: 0.0045,
    });
  });

  it("reaches the same phase through one or many updates", () => {
    const one = computeMotionFrame(1400);
    const many = Array.from({ length: 14 }).reduce<number>((elapsed) => elapsed + 100, 0);
    expect(computeMotionFrame(many)).toEqual(one);
  });

  it("keeps breath and sway inside the approved envelope", () => {
    for (let elapsed = 0; elapsed <= 10400; elapsed += 50) {
      const frame = computeMotionFrame(elapsed);
      expect(frame.breath).toBeGreaterThanOrEqual(0);
      expect(frame.breath).toBeLessThanOrEqual(1);
      expect(Math.abs(frame.swayRadians)).toBeLessThanOrEqual(0.7 * Math.PI / 180);
      expect(Math.abs(frame.swayXRatio)).toBeLessThanOrEqual(0.0045);
    }
  });

  it("plans a dense uniform envelope along the correlated sway trajectory", () => {
    const transforms = planHitEnvelopeTransforms(defaultHitGeometry);
    const normalized = transforms.map((transform) =>
      transform.swayXRatio / LIFE_V1.swayXRatio,
    );

    expect(normalized[0]).toBe(-1);
    expect(normalized.at(-1)).toBe(1);
    expect(Math.max(...normalized.slice(1).map((value, index) =>
      value - normalized[index]!,
    ))).toBeLessThanOrEqual(1 / 16 + 1e-12);
    for (const transform of transforms) {
      expect(transform.swayRadians / LIFE_V1.swayRadians).toBeCloseTo(
        transform.swayXRatio / LIFE_V1.swayXRatio,
      );
    }
  });

  it("includes representative real display phases in the hit envelope plan", () => {
    const transforms = planHitEnvelopeTransforms(defaultHitGeometry);
    for (const elapsedMs of [0, 5200 / 12, 1300, 5200 * 7 / 12, 3900]) {
      const frame = computeMotionFrame(elapsedMs);
      expect(transforms.some((transform) =>
        Math.abs(transform.swayXRatio - frame.swayXRatio) < 1e-12
        && Math.abs(transform.swayRadians - frame.swayRadians) < 1e-12
      )).toBe(true);
    }
  });

  it("keeps adjacent envelope samples within one physical pixel on a large viewport", () => {
    const geometry = {
      viewportWidth: 3840,
      dpr: 3,
      bounds: { x: 840, y: 0, width: 2160, height: 2160 },
      pivot: { x: 1920, y: 1555.2 },
    };
    const transforms = planHitEnvelopeTransforms(geometry);
    const corners = [
      [geometry.bounds.x, geometry.bounds.y],
      [geometry.bounds.x + geometry.bounds.width, geometry.bounds.y],
      [geometry.bounds.x, geometry.bounds.y + geometry.bounds.height],
      [geometry.bounds.x + geometry.bounds.width, geometry.bounds.y + geometry.bounds.height],
    ];
    const rMax = Math.max(...corners.map(([x, y]) =>
      Math.hypot(x! - geometry.pivot.x, y! - geometry.pivot.y),
    ));
    const physicalSteps = transforms.slice(1).map((transform, index) => {
      const previous = transforms[index]!;
      return geometry.dpr * (
        geometry.viewportWidth * Math.abs(transform.swayXRatio - previous.swayXRatio)
        + rMax * Math.abs(transform.swayRadians - previous.swayRadians)
      );
    });

    expect(transforms.length).toBeGreaterThanOrEqual(33);
    expect(Number.isInteger(transforms.length)).toBe(true);
    expect(Math.max(...physicalSteps)).toBeLessThanOrEqual(1 + 1e-12);
  });

  it("zeros both breath-zone seams and peaks in the middle", () => {
    expect(breathWeight(0)).toBeCloseTo(0);
    expect(breathWeight(0.5)).toBeCloseTo(1);
    expect(breathWeight(1)).toBeCloseTo(0);
  });

  it("never locally deforms slices above the breath zone", () => {
    const slices = planBreathSlices(validMotionProfile(), 1000, 1000, 0.25, 24);
    for (const slice of slices.filter((value) => value.sourceY + value.sourceHeight <= 500)) {
      expect(slice.destX).toBe(slice.sourceX);
      expect(slice.destY).toBe(slice.sourceY);
      expect(slice.destWidth).toBe(slice.sourceWidth);
      expect(slice.destHeight).toBe(slice.sourceHeight);
    }
  });

  it("forces both breath-zone boundaries into the slice plan", () => {
    const slices = planBreathSlices(validMotionProfile(), 997, 991, 1, 24);
    const horizontalEdges = new Set(slices.flatMap((slice) => [
      slice.sourceY,
      slice.sourceY + slice.sourceHeight,
    ]));
    expect(horizontalEdges.has(991 * 0.5)).toBe(true);
    expect(horizontalEdges.has(991 * 0.84)).toBe(true);
    expect(slices.every((slice) => !(
      slice.sourceY < 991 * 0.5
      && slice.sourceY + slice.sourceHeight > 991 * 0.5
    ))).toBe(true);
  });

  it("keeps pixels beside the chest zone unchanged", () => {
    const slices = planBreathSlices(validMotionProfile(), 1000, 1000, 1, 24);
    const besideChest = slices.filter((slice) =>
      slice.sourceX + slice.sourceWidth <= 200 || slice.sourceX >= 800,
    );
    expect(besideChest.length).toBeGreaterThan(0);
    expect(besideChest.every((slice) =>
      slice.destX === slice.sourceX
      && slice.destY === slice.sourceY
      && slice.destWidth === slice.sourceWidth
      && slice.destHeight === slice.sourceHeight
    )).toBe(true);
  });

  it("keeps both horizontal breath-zone seams free of local widening", () => {
    const slices = planBreathSlices(validMotionProfile(), 1000, 1000, 1, 24);
    const central = slices.filter((slice) => slice.sourceX === 200 && slice.sourceWidth === 600);
    const atTopSeam = central.find((slice) => slice.sourceY === 500);
    const atBottomSeam = central.find((slice) => slice.sourceY + slice.sourceHeight === 840);

    expect(atTopSeam).toMatchObject({ destX: 200, destWidth: 600, destY: 500 });
    expect(atBottomSeam!.destX).toBe(200);
    expect(atBottomSeam!.destWidth).toBe(600);
    expect(atBottomSeam!.destY + atBottomSeam!.destHeight).toBeCloseTo(840);
  });
});
