import { describe, expect, it } from "vitest";
import {
  canonicalPetCalibration,
  DEFAULT_PET_CALIBRATION,
  type PetCalibrationV1,
} from "./pet-calibration";

function calibration(overrides: Partial<PetCalibrationV1> = {}): PetCalibrationV1 {
  return { ...DEFAULT_PET_CALIBRATION, ...overrides };
}

describe("pet calibration contract", () => {
  it("keeps the dormant blink field for persisted-data compatibility", () => {
    expect(canonicalPetCalibration(calibration({ blinkIntervalScale: 1.5 })))
      .toEqual(calibration({ blinkIntervalScale: 1.5 }));
  });

  it.each([
    ["schema version", { schemaVersion: 2 }],
    ["negative breath", { breathAmplitudePercent: -1 }],
    ["oversized breath", { breathAmplitudePercent: 5.01 }],
    ["non-finite breath", { breathAmplitudePercent: Number.NaN }],
    ["infinite breath", { breathAmplitudePercent: Number.POSITIVE_INFINITY }],
    ["zero dormant blink scale", { blinkIntervalScale: 0 }],
    ["oversized dormant blink scale", { blinkIntervalScale: 2.01 }],
    ["non-finite dormant blink scale", { blinkIntervalScale: Number.NaN }],
    ["negative feedback", { feedbackStrength: -0.01 }],
    ["oversized feedback", { feedbackStrength: 1.01 }],
    ["non-finite feedback", { feedbackStrength: Number.NaN }],
  ])("rejects invalid %s", (_name, overrides) => {
    expect(() => canonicalPetCalibration({
      ...DEFAULT_PET_CALIBRATION,
      ...overrides,
    })).toThrow(/calibration|schemaVersion/i);
  });
});
