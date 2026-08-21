export interface PetCalibrationV1 {
  schemaVersion: 1;
  breathAmplitudePercent: number;
  blinkIntervalScale: number;
  feedbackStrength: number;
}

export const DEFAULT_PET_CALIBRATION: Readonly<PetCalibrationV1> = Object.freeze({
  schemaVersion: 1,
  breathAmplitudePercent: 2,
  blinkIntervalScale: 1,
  feedbackStrength: 0.6,
});

export function canonicalPetCalibration(value: unknown): PetCalibrationV1 {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("pet calibration must be an object");
  }
  const candidate = value as Record<string, unknown>;
  if (candidate.schemaVersion !== 1) {
    throw new Error(`unsupported pet calibration schemaVersion: ${String(candidate.schemaVersion)}`);
  }
  const breathAmplitudePercent = calibrationNumber(
    candidate.breathAmplitudePercent,
    "breathAmplitudePercent",
    0,
    5,
  );
  const blinkIntervalScale = calibrationNumber(
    candidate.blinkIntervalScale,
    "blinkIntervalScale",
    0.5,
    2,
  );
  const feedbackStrength = calibrationNumber(candidate.feedbackStrength, "feedbackStrength", 0, 1);
  return {
    schemaVersion: 1,
    breathAmplitudePercent,
    blinkIntervalScale,
    feedbackStrength,
  };
}

function calibrationNumber(value: unknown, field: string, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new RangeError(`pet calibration ${field} must be finite`);
  }
  if (value < min || value > max) {
    throw new RangeError(`pet calibration ${field} must be between ${min} and ${max}`);
  }
  return value;
}
