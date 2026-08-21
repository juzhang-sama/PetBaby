import type { MotionProfileV1 } from "./animated-image-manifest";

export const LIFE_V1 = Object.freeze({
  breathPeriodMs: 2800,
  breathScaleX: 0.028,
  breathShiftY: 0.012,
  swayPeriodMs: 5200,
  swayRadians: 0.7 * Math.PI / 180,
  swayXRatio: 0.0045,
});

export interface MotionFrame {
  breath: number;
  swayRadians: number;
  swayXRatio: number;
}

export type FullImageAction =
  | "idle"
  | "look-left"
  | "look-right"
  | "react-happy"
  | "react-curious"
  | "sleep"
  | "wake"
  | "carried"
  | "landed";

export type FullImageActionFrame = {
  readonly translateXRatio: number;
  readonly translateYRatio: number;
  readonly scaleX: number;
  readonly scaleY: number;
  readonly rotationRadians: number;
  readonly completed: boolean;
};

const ACTION_DURATION_MS = {
  "look-left": 900,
  "look-right": 900,
  "react-happy": 760,
  "react-curious": 1100,
  sleep: 1400,
  wake: 720,
  carried: 1200,
  landed: 520,
} as const;

export function computeFullImageActionFrame(
  action: FullImageAction,
  elapsedMs: number,
  loop: boolean,
): FullImageActionFrame {
  if (action === "idle") return actionFrame(0, 0, 1, 1, 0, false);
  const duration = ACTION_DURATION_MS[action];
  const progress = Math.min(1, Math.max(0, elapsedMs / duration));
  const phase = loop ? elapsedMs % duration / duration : progress;
  const pulse = Math.sin(Math.PI * phase);
  const completed = !loop && progress >= 1;
  switch (action) {
    case "look-left":
      return actionFrame(-0.018 * pulse, 0, 1, 1, -0.025 * pulse, completed);
    case "look-right":
      return actionFrame(0.018 * pulse, 0, 1, 1, 0.025 * pulse, completed);
    case "react-happy":
      return actionFrame(0, -0.055 * Math.abs(Math.sin(2 * Math.PI * phase)), 1.025, 0.975, 0, completed);
    case "react-curious":
      return actionFrame(0.012 * pulse, -0.008 * pulse, 1, 1, 0.07 * pulse, completed);
    case "sleep":
      return actionFrame(0, 0.018 * progress, 1.02, 0.96, -0.025 * progress, completed);
    case "wake":
      return actionFrame(0, -0.025 * pulse, 1 - 0.02 * pulse, 1 + 0.04 * pulse, 0, completed);
    case "carried":
      return actionFrame(0.018 * Math.sin(2 * Math.PI * phase), -0.02, 1, 1, 0.035 * Math.sin(2 * Math.PI * phase), completed);
    case "landed":
      return actionFrame(0, 0.025 * pulse, 1 + 0.055 * pulse, 1 - 0.08 * pulse, 0, completed);
    default:
      return assertNeverAction(action);
  }
}

function actionFrame(
  translateXRatio: number,
  translateYRatio: number,
  scaleX: number,
  scaleY: number,
  rotationRadians: number,
  completed: boolean,
): FullImageActionFrame {
  return { translateXRatio, translateYRatio, scaleX, scaleY, rotationRadians, completed };
}

function assertNeverAction(value: never): never {
  throw new Error(`unsupported full-image action: ${String(value)}`);
}

export interface HitEnvelopeTransform {
  swayXRatio: number;
  swayRadians: number;
}

export interface HitEnvelopeGeometry {
  viewportWidth: number;
  dpr: number;
  bounds: { x: number; y: number; width: number; height: number };
  pivot: { x: number; y: number };
}

export interface BreathSlice {
  sourceX: number;
  sourceY: number;
  sourceWidth: number;
  sourceHeight: number;
  destX: number;
  destY: number;
  destWidth: number;
  destHeight: number;
}

export function planRasterSafeBreathSlice(
  slice: BreathSlice,
  imageWidth: number,
  imageHeight: number,
  destinationScale: number,
  dpr: number,
): BreathSlice {
  const destinationBleed = 0.5 / (destinationScale * dpr);
  const sourceBleedX = destinationBleed * slice.sourceWidth / slice.destWidth;
  const sourceBleedY = destinationBleed * slice.sourceHeight / slice.destHeight;
  const epsilon = Number.EPSILON * Math.max(1, imageWidth, imageHeight) * 4;
  const bleedLeft = slice.sourceX > epsilon;
  const bleedRight = slice.sourceX + slice.sourceWidth < imageWidth - epsilon;
  const bleedTop = slice.sourceY > epsilon;
  const bleedBottom = slice.sourceY + slice.sourceHeight < imageHeight - epsilon;

  return {
    sourceX: slice.sourceX - (bleedLeft ? sourceBleedX : 0),
    sourceY: slice.sourceY - (bleedTop ? sourceBleedY : 0),
    sourceWidth: slice.sourceWidth
      + (bleedLeft ? sourceBleedX : 0)
      + (bleedRight ? sourceBleedX : 0),
    sourceHeight: slice.sourceHeight
      + (bleedTop ? sourceBleedY : 0)
      + (bleedBottom ? sourceBleedY : 0),
    destX: slice.destX - (bleedLeft ? destinationBleed : 0),
    destY: slice.destY - (bleedTop ? destinationBleed : 0),
    destWidth: slice.destWidth
      + (bleedLeft ? destinationBleed : 0)
      + (bleedRight ? destinationBleed : 0),
    destHeight: slice.destHeight
      + (bleedTop ? destinationBleed : 0)
      + (bleedBottom ? destinationBleed : 0),
  };
}

export function computeMotionFrame(elapsedMs: number): MotionFrame {
  const breathPhase = 2 * Math.PI
    * (elapsedMs % LIFE_V1.breathPeriodMs)
    / LIFE_V1.breathPeriodMs;
  const swayPhase = 2 * Math.PI
    * (elapsedMs % LIFE_V1.swayPeriodMs)
    / LIFE_V1.swayPeriodMs;
  return {
    breath: (Math.sin(breathPhase - Math.PI / 2) + 1) / 2,
    swayRadians: Math.sin(swayPhase) * LIFE_V1.swayRadians,
    swayXRatio: Math.sin(swayPhase) * LIFE_V1.swayXRatio,
  };
}

export function planHitEnvelopeTransforms(
  geometry: HitEnvelopeGeometry,
): HitEnvelopeTransform[] {
  const { bounds, pivot } = geometry;
  const rMax = Math.max(
    Math.hypot(bounds.x - pivot.x, bounds.y - pivot.y),
    Math.hypot(bounds.x + bounds.width - pivot.x, bounds.y - pivot.y),
    Math.hypot(bounds.x - pivot.x, bounds.y + bounds.height - pivot.y),
    Math.hypot(bounds.x + bounds.width - pivot.x, bounds.y + bounds.height - pivot.y),
  );
  const physicalHalfTravel = geometry.dpr * (
    geometry.viewportWidth * LIFE_V1.swayXRatio
    + rMax * LIFE_V1.swayRadians
  );
  const intervalCount = Math.max(32, Math.ceil(2 * physicalHalfTravel));
  const transforms = Array.from({ length: intervalCount + 1 }, (_, index) => {
    const scalar = -1 + 2 * index / intervalCount;
    return {
      swayXRatio: scalar * LIFE_V1.swayXRatio,
      swayRadians: scalar * LIFE_V1.swayRadians,
    };
  });
  if (intervalCount % 2 !== 0) {
    transforms.splice((intervalCount + 1) / 2, 0, { swayXRatio: 0, swayRadians: 0 });
  }
  return transforms;
}

export function breathWeight(normalizedY: number): number {
  const t = Math.min(1, Math.max(0, normalizedY));
  return Math.sin(Math.PI * t);
}

export function planBreathSlices(
  profile: MotionProfileV1,
  imageWidth: number,
  imageHeight: number,
  breath: number,
  sliceCount: number,
  breathAmplitudePercent = 2,
): BreathSlice[] {
  if (imageWidth <= 0 || imageHeight <= 0) {
    throw new RangeError("image dimensions must be positive");
  }
  if (!Number.isInteger(sliceCount) || sliceCount <= 0) {
    throw new RangeError("slice count must be a positive integer");
  }

  const zoneLeft = profile.breathZone.left * imageWidth;
  const zoneRight = profile.breathZone.right * imageWidth;
  const zoneTop = profile.breathZone.top * imageHeight;
  const zoneBottom = profile.breathZone.bottom * imageHeight;
  const faceSafetyLine = (
    profile.alphaBounds.top
    + (profile.alphaBounds.bottom - profile.alphaBounds.top) * 0.4
  ) * imageHeight;
  const yBoundaries = Array.from({ length: sliceCount + 1 }, (_, index) =>
    imageHeight * index / sliceCount,
  );
  yBoundaries.push(faceSafetyLine, zoneTop, zoneBottom);
  yBoundaries.sort((a, b) => a - b);

  const uniqueYBoundaries = yBoundaries.filter((value, index) =>
    index === 0 || Math.abs(value - yBoundaries[index - 1]!) > Number.EPSILON,
  );
  const xBoundaries = [0, zoneLeft, zoneRight, imageWidth];
  const slices: BreathSlice[] = [];

  for (let yIndex = 0; yIndex < uniqueYBoundaries.length - 1; yIndex += 1) {
    const sourceY = uniqueYBoundaries[yIndex]!;
    const sourceBottom = uniqueYBoundaries[yIndex + 1]!;
    const sourceHeight = sourceBottom - sourceY;
    for (let xIndex = 0; xIndex < xBoundaries.length - 1; xIndex += 1) {
      const sourceX = xBoundaries[xIndex]!;
      const sourceWidth = xBoundaries[xIndex + 1]! - sourceX;
      const centralChest = xIndex === 1 && sourceY >= zoneTop && sourceBottom <= zoneBottom;
      if (!centralChest) {
        slices.push({
          sourceX,
          sourceY,
          sourceWidth,
          sourceHeight,
          destX: sourceX,
          destY: sourceY,
          destWidth: sourceWidth,
          destHeight: sourceHeight,
        });
        continue;
      }

      const normalizedTop = (sourceY - zoneTop) / (zoneBottom - zoneTop);
      const normalizedBottom = (sourceBottom - zoneTop) / (zoneBottom - zoneTop);
      const normalizedMiddle = (normalizedTop + normalizedBottom) / 2;
      const horizontalWeight = normalizedTop <= 0 || normalizedBottom >= 1
        ? 0
        : breathWeight(normalizedMiddle);
      const calibratedBreath = breath * breathAmplitudePercent / 2;
      const topShift = imageHeight * LIFE_V1.breathShiftY * calibratedBreath * breathWeight(normalizedTop);
      const bottomShift = imageHeight * LIFE_V1.breathShiftY * calibratedBreath * breathWeight(normalizedBottom);
      const widthIncrease = sourceWidth * LIFE_V1.breathScaleX * calibratedBreath * horizontalWeight;
      slices.push({
        sourceX,
        sourceY,
        sourceWidth,
        sourceHeight,
        destX: sourceX - widthIncrease / 2,
        destY: sourceY + topShift,
        destWidth: sourceWidth + widthIncrease,
        destHeight: sourceHeight + bottomShift - topShift,
      });
    }
  }
  return slices;
}
