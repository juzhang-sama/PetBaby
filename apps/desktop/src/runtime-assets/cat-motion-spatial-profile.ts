export const BODY_MODULE_IDS_V1 = [
  "body-slender-v1",
  "body-balanced-v1",
  "body-rounded-v1",
] as const;

export type BodyModuleIdV1 = (typeof BODY_MODULE_IDS_V1)[number];

export interface NormalizedPointV1 {
  x: number;
  y: number;
}

export interface NormalizedRectV1 {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface MotionAmplitudeRangeV1 {
  min: number;
  max: number;
}

export type CatMotionAmplitudeSemanticV1 =
  | "breath"
  | "blink"
  | "ear"
  | "tailAngle"
  | "tailCurl"
  | "tailTip"
  | "bodyStretch";

export interface MotionSpatialProfileV1 {
  schemaVersion: 1;
  bodyModuleId: BodyModuleIdV1;
  canvas: { width: number; height: number };
  alphaBounds: NormalizedRectV1;
  faceSafeZone: NormalizedRectV1;
  eyes: {
    left: { center: NormalizedPointV1; bounds: NormalizedRectV1 };
    right: { center: NormalizedPointV1; bounds: NormalizedRectV1 };
  };
  earRoots: { left: NormalizedPointV1; right: NormalizedPointV1 };
  breathZone: NormalizedRectV1;
  stretchAxis: { origin: NormalizedPointV1; direction: NormalizedPointV1 };
  swayPivot: NormalizedPointV1;
  tailRoot: NormalizedPointV1;
  edgeTailBounds: NormalizedRectV1;
  amplitude: Record<CatMotionAmplitudeSemanticV1, MotionAmplitudeRangeV1>;
}

const BODY_MODULE_IDS = new Set<string>(BODY_MODULE_IDS_V1);
const AMPLITUDE_SEMANTICS: readonly CatMotionAmplitudeSemanticV1[] = [
  "breath",
  "blink",
  "ear",
  "tailAngle",
  "tailCurl",
  "tailTip",
  "bodyStretch",
];

export function parseMotionSpatialProfileV1(input: unknown): MotionSpatialProfileV1 {
  const value = exactObject(input, "profile", [
    "schemaVersion",
    "bodyModuleId",
    "canvas",
    "alphaBounds",
    "faceSafeZone",
    "eyes",
    "earRoots",
    "breathZone",
    "stretchAxis",
    "swayPivot",
    "tailRoot",
    "edgeTailBounds",
    "amplitude",
  ]);
  if (value.schemaVersion !== 1) fail("schemaVersion must be 1");
  if (typeof value.bodyModuleId !== "string" || !BODY_MODULE_IDS.has(value.bodyModuleId)) {
    fail("bodyModuleId is not a supported body module");
  }

  const canvasValue = exactObject(value.canvas, "canvas", ["width", "height"]);
  const canvas = {
    width: positiveInteger(canvasValue.width, "canvas.width"),
    height: positiveInteger(canvasValue.height, "canvas.height"),
  };
  const alphaBounds = normalizedRect(value.alphaBounds, "alphaBounds");
  const faceSafeZone = normalizedRect(value.faceSafeZone, "faceSafeZone");
  const eyesValue = exactObject(value.eyes, "eyes", ["left", "right"]);
  const eyes = {
    left: eye(eyesValue.left, "eyes.left"),
    right: eye(eyesValue.right, "eyes.right"),
  };
  const earRootsValue = exactObject(value.earRoots, "earRoots", ["left", "right"]);
  const earRoots = {
    left: normalizedPoint(earRootsValue.left, "earRoots.left"),
    right: normalizedPoint(earRootsValue.right, "earRoots.right"),
  };
  const breathZone = normalizedRect(value.breathZone, "breathZone");
  const stretchAxisValue = exactObject(value.stretchAxis, "stretchAxis", ["origin", "direction"]);
  const stretchAxis = {
    origin: normalizedPoint(stretchAxisValue.origin, "stretchAxis.origin"),
    direction: normalizedPoint(stretchAxisValue.direction, "stretchAxis.direction"),
  };
  const swayPivot = normalizedPoint(value.swayPivot, "swayPivot");
  const tailRoot = normalizedPoint(value.tailRoot, "tailRoot");
  const edgeTailBounds = normalizedRect(value.edgeTailBounds, "edgeTailBounds");
  const amplitudeValue = exactObject(value.amplitude, "amplitude", AMPLITUDE_SEMANTICS);
  const amplitude = {
    breath: amplitudeRange(amplitudeValue.breath, "amplitude.breath"),
    blink: amplitudeRange(amplitudeValue.blink, "amplitude.blink"),
    ear: amplitudeRange(amplitudeValue.ear, "amplitude.ear"),
    tailAngle: amplitudeRange(amplitudeValue.tailAngle, "amplitude.tailAngle"),
    tailCurl: amplitudeRange(amplitudeValue.tailCurl, "amplitude.tailCurl"),
    tailTip: amplitudeRange(amplitudeValue.tailTip, "amplitude.tailTip"),
    bodyStretch: amplitudeRange(amplitudeValue.bodyStretch, "amplitude.bodyStretch"),
  };

  requireRectInside(faceSafeZone, alphaBounds, "faceSafeZone", "alphaBounds");
  for (const side of ["left", "right"] as const) {
    requireRectInside(eyes[side].bounds, faceSafeZone, `eyes.${side}.bounds`, "faceSafeZone");
    requirePointInside(eyes[side].center, eyes[side].bounds, `eyes.${side}.center`, `eyes.${side}.bounds`);
  }
  if (
    eyes.left.center.x >= eyes.right.center.x
    || eyes.left.bounds.left >= eyes.right.bounds.left
  ) {
    fail("eyes must preserve left-to-right ordering");
  }

  requireRectInside(breathZone, alphaBounds, "breathZone", "alphaBounds");
  if (
    positiveAreaOverlap(breathZone, eyes.left.bounds)
    || positiveAreaOverlap(breathZone, eyes.right.bounds)
  ) {
    fail("breathZone must not overlap eyes with positive area");
  }

  for (const side of ["left", "right"] as const) {
    requirePointInside(earRoots[side], alphaBounds, `earRoots.${side}`, "alphaBounds");
    if (earRoots[side].y > faceSafeZone.bottom) {
      fail(`earRoots.${side} must remain in the subject upper region`);
    }
  }
  if (earRoots.left.x >= earRoots.right.x) fail("earRoots must preserve left-to-right ordering");

  requirePointInside(stretchAxis.origin, alphaBounds, "stretchAxis.origin", "alphaBounds");
  if (stretchAxis.direction.x === 0 && stretchAxis.direction.y === 0) {
    fail("stretchAxis.direction must be non-zero");
  }
  requirePointInside(swayPivot, alphaBounds, "swayPivot", "alphaBounds");
  requireRectInside(edgeTailBounds, alphaBounds, "edgeTailBounds", "alphaBounds");
  requirePointInside(tailRoot, alphaBounds, "tailRoot", "alphaBounds");
  requirePointInside(tailRoot, edgeTailBounds, "tailRoot", "edgeTailBounds");

  return {
    schemaVersion: 1,
    bodyModuleId: value.bodyModuleId as BodyModuleIdV1,
    canvas,
    alphaBounds,
    faceSafeZone,
    eyes,
    earRoots,
    breathZone,
    stretchAxis,
    swayPivot,
    tailRoot,
    edgeTailBounds,
    amplitude,
  };
}

export function clampCatMotionValue(
  profile: MotionSpatialProfileV1,
  semantic: CatMotionAmplitudeSemanticV1,
  value: number,
): number {
  if (!Number.isFinite(value)) fail("motion value must be finite");
  const range = profile.amplitude[semantic];
  return Math.min(range.max, Math.max(range.min, value));
}

function eye(value: unknown, path: string): { center: NormalizedPointV1; bounds: NormalizedRectV1 } {
  const object = exactObject(value, path, ["center", "bounds"]);
  return {
    center: normalizedPoint(object.center, `${path}.center`),
    bounds: normalizedRect(object.bounds, `${path}.bounds`),
  };
}

function normalizedPoint(value: unknown, path: string): NormalizedPointV1 {
  const object = exactObject(value, path, ["x", "y"]);
  return {
    x: normalizedNumber(object.x, `${path}.x`),
    y: normalizedNumber(object.y, `${path}.y`),
  };
}

function normalizedRect(value: unknown, path: string): NormalizedRectV1 {
  const object = exactObject(value, path, ["left", "top", "right", "bottom"]);
  const rect = {
    left: normalizedNumber(object.left, `${path}.left`),
    top: normalizedNumber(object.top, `${path}.top`),
    right: normalizedNumber(object.right, `${path}.right`),
    bottom: normalizedNumber(object.bottom, `${path}.bottom`),
  };
  if (rect.left >= rect.right || rect.top >= rect.bottom) fail(`${path} must have positive area`);
  return rect;
}

function amplitudeRange(value: unknown, path: string): MotionAmplitudeRangeV1 {
  const object = exactObject(value, path, ["min", "max"]);
  const min = finiteNumber(object.min, `${path}.min`);
  const max = finiteNumber(object.max, `${path}.max`);
  if (min >= max) fail(`${path}.min must be less than max`);
  return { min, max };
}

function exactObject(value: unknown, path: string, keys: readonly string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    fail(`${path} must be an object`);
  }
  const object = value as Record<string, unknown>;
  const allowed = new Set(keys);
  const unknown = Object.keys(object).find((key) => !allowed.has(key));
  if (unknown !== undefined) fail(`${path} has unknown field "${unknown}"`);
  return object;
}

function normalizedNumber(value: unknown, path: string): number {
  const number = finiteNumber(value, path);
  if (number < 0 || number > 1) fail(`${path} must be within [0, 1]`);
  return number;
}

function finiteNumber(value: unknown, path: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) fail(`${path} must be finite`);
  return value;
}

function positiveInteger(value: unknown, path: string): number {
  const number = finiteNumber(value, path);
  if (!Number.isInteger(number) || number <= 0) fail(`${path} must be a positive integer`);
  return number;
}

function requireRectInside(
  inner: NormalizedRectV1,
  outer: NormalizedRectV1,
  innerPath: string,
  outerPath: string,
): void {
  if (
    inner.left < outer.left
    || inner.top < outer.top
    || inner.right > outer.right
    || inner.bottom > outer.bottom
  ) {
    fail(`${innerPath} must remain inside ${outerPath}`);
  }
}

function requirePointInside(
  point: NormalizedPointV1,
  rect: NormalizedRectV1,
  pointPath: string,
  rectPath: string,
): void {
  if (
    point.x < rect.left
    || point.x > rect.right
    || point.y < rect.top
    || point.y > rect.bottom
  ) {
    fail(`${pointPath} must remain inside ${rectPath}`);
  }
}

function positiveAreaOverlap(left: NormalizedRectV1, right: NormalizedRectV1): boolean {
  return Math.min(left.right, right.right) > Math.max(left.left, right.left)
    && Math.min(left.bottom, right.bottom) > Math.max(left.top, right.top);
}

function fail(reason: string): never {
  throw new TypeError(`invalid MotionSpatialProfileV1: ${reason}`);
}
