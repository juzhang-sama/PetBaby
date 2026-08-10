import { describe, expect, it } from "vitest";
import { validMotionProfile } from "./animated-image-test-fixtures";
import {
  LIFE_V1,
  breathWeight,
  computeMotionFrame,
  planBreathSlices,
} from "./animated-image-motion";

describe("animated image motion", () => {
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
