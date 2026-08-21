import { describe, expect, it } from "vitest";
import {
  BODY_MODULE_IDS_V1,
  clampCatMotionValue,
  parseMotionSpatialProfileV1,
  type MotionSpatialProfileV1,
} from "./cat-motion-spatial-profile";

function validProfile(): MotionSpatialProfileV1 {
  return {
    schemaVersion: 1,
    bodyModuleId: "body-balanced-v1",
    canvas: { width: 1_000, height: 1_200 },
    alphaBounds: { left: 0.1, top: 0.05, right: 0.9, bottom: 0.95 },
    faceSafeZone: { left: 0.25, top: 0.1, right: 0.75, bottom: 0.4 },
    eyes: {
      left: {
        center: { x: 0.38, y: 0.25 },
        bounds: { left: 0.32, top: 0.18, right: 0.44, bottom: 0.32 },
      },
      right: {
        center: { x: 0.62, y: 0.25 },
        bounds: { left: 0.56, top: 0.18, right: 0.68, bottom: 0.32 },
      },
    },
    earRoots: {
      left: { x: 0.24, y: 0.13 },
      right: { x: 0.76, y: 0.13 },
    },
    breathZone: { left: 0.3, top: 0.45, right: 0.7, bottom: 0.75 },
    stretchAxis: {
      origin: { x: 0.5, y: 0.65 },
      direction: { x: 0, y: 1 },
    },
    swayPivot: { x: 0.5, y: 0.7 },
    tailRoot: { x: 0.78, y: 0.72 },
    edgeTailBounds: { left: 0.74, top: 0.55, right: 0.9, bottom: 0.9 },
    amplitude: {
      breath: { min: 0, max: 1 },
      blink: { min: 0, max: 1 },
      ear: { min: -0.35, max: 0.35 },
      tailAngle: { min: -20, max: 20 },
      tailCurl: { min: -0.6, max: 0.6 },
      tailTip: { min: -0.7, max: 0.7 },
      bodyStretch: { min: 0, max: 1 },
    },
  };
}

function cloneProfile(): Record<string, unknown> {
  return structuredClone(validProfile()) as unknown as Record<string, unknown>;
}

describe("MotionSpatialProfileV1", () => {
  it("freezes the exact three body module identifiers", () => {
    expect(BODY_MODULE_IDS_V1).toEqual([
      "body-slender-v1",
      "body-balanced-v1",
      "body-rounded-v1",
    ]);
  });

  it("parses a complete profile and returns an independent canonical value", () => {
    const input = validProfile();
    const parsed = parseMotionSpatialProfileV1(input);

    expect(parsed).toEqual(input);
    expect(parsed).not.toBe(input);
    expect(parsed.eyes).not.toBe(input.eyes);
    expect(parsed.amplitude).not.toBe(input.amplitude);
  });

  it("rejects unknown root and nested fields", () => {
    const rootExtra = { ...validProfile(), debug: true };
    expect(() => parseMotionSpatialProfileV1(rootExtra)).toThrow(/unknown field.*debug/i);

    const nestedExtra = cloneProfile();
    const canvas = nestedExtra.canvas as Record<string, unknown>;
    canvas.depth = 1;
    expect(() => parseMotionSpatialProfileV1(nestedExtra)).toThrow(/canvas.*unknown field.*depth/i);
  });

  it("rejects unknown modules and invalid canvas dimensions", () => {
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      bodyModuleId: "body-unknown-v1",
    })).toThrow(/bodyModuleId/i);
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      canvas: { width: 0, height: 1_200 },
    })).toThrow(/canvas\.width/i);
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      canvas: { width: 1_000.5, height: 1_200 },
    })).toThrow(/canvas\.width/i);
  });

  it("requires every normalized point and rectangle to stay in range with positive area", () => {
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      swayPivot: { x: 1.01, y: 0.7 },
    })).toThrow(/swayPivot\.x/i);
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      alphaBounds: { left: 0.1, top: 0.05, right: 0.1, bottom: 0.95 },
    })).toThrow(/alphaBounds.*positive area/i);
  });

  it("keeps the face zone and both eyes inside the subject with left-to-right ordering", () => {
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      faceSafeZone: { left: 0.05, top: 0.1, right: 0.75, bottom: 0.4 },
    })).toThrow(/faceSafeZone.*alphaBounds/i);

    const outsideEye = validProfile();
    outsideEye.eyes.left.bounds.left = 0.2;
    expect(() => parseMotionSpatialProfileV1(outsideEye)).toThrow(/eyes\.left\.bounds.*faceSafeZone/i);

    const reversedEyes = validProfile();
    const leftEye = reversedEyes.eyes.left;
    reversedEyes.eyes.left = reversedEyes.eyes.right;
    reversedEyes.eyes.right = leftEye;
    expect(() => parseMotionSpatialProfileV1(reversedEyes)).toThrow(/left-to-right/i);
  });

  it("keeps breath below the eyes and fully inside the subject", () => {
    const eyeOverlap = validProfile();
    eyeOverlap.breathZone = { left: 0.35, top: 0.3, right: 0.65, bottom: 0.6 };
    expect(() => parseMotionSpatialProfileV1(eyeOverlap)).toThrow(/breathZone.*eyes/i);

    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      breathZone: { left: 0.3, top: 0.45, right: 0.95, bottom: 0.75 },
    })).toThrow(/breathZone.*alphaBounds/i);
  });

  it("requires upper ear roots, anchored pivots and a non-zero stretch direction", () => {
    const lowEar = validProfile();
    lowEar.earRoots.left.y = 0.6;
    expect(() => parseMotionSpatialProfileV1(lowEar)).toThrow(/earRoots\.left.*upper/i);

    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      stretchAxis: { origin: { x: 0.5, y: 0.65 }, direction: { x: 0, y: 0 } },
    })).toThrow(/stretchAxis\.direction.*non-zero/i);
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      swayPivot: { x: 0.95, y: 0.7 },
    })).toThrow(/swayPivot.*alphaBounds/i);
  });

  it("anchors the tail root to both the subject and independent tail region", () => {
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      tailRoot: { x: 0.7, y: 0.72 },
    })).toThrow(/tailRoot.*edgeTailBounds/i);
    expect(() => parseMotionSpatialProfileV1({
      ...validProfile(),
      edgeTailBounds: { left: 0.74, top: 0.55, right: 0.98, bottom: 0.9 },
    })).toThrow(/edgeTailBounds.*alphaBounds/i);
  });

  it("requires finite increasing amplitude ranges", () => {
    const equalRange = validProfile();
    equalRange.amplitude.tailCurl = { min: 0.2, max: 0.2 };
    expect(() => parseMotionSpatialProfileV1(equalRange)).toThrow(/amplitude\.tailCurl.*min.*max/i);

    const nonFinite = validProfile();
    nonFinite.amplitude.ear.max = Number.POSITIVE_INFINITY;
    expect(() => parseMotionSpatialProfileV1(nonFinite)).toThrow(/amplitude\.ear\.max.*finite/i);
  });

  it("clamps every motion semantic to its character range", () => {
    const profile = parseMotionSpatialProfileV1(validProfile());

    expect(clampCatMotionValue(profile, "breath", -1)).toBe(0);
    expect(clampCatMotionValue(profile, "blink", 0.4)).toBe(0.4);
    expect(clampCatMotionValue(profile, "ear", 1)).toBe(0.35);
    expect(clampCatMotionValue(profile, "tailAngle", -100)).toBe(-20);
    expect(clampCatMotionValue(profile, "tailCurl", 100)).toBe(0.6);
    expect(clampCatMotionValue(profile, "tailTip", -100)).toBe(-0.7);
    expect(clampCatMotionValue(profile, "bodyStretch", 4)).toBe(1);
    expect(() => clampCatMotionValue(profile, "breath", Number.NaN)).toThrow(/finite/i);
  });
});
