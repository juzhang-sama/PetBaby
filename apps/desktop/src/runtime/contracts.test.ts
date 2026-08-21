import { describe, expect, it } from "vitest";
import {
  isProbePreferences,
  PREFERENCES_SCHEMA_VERSION,
  PROBE_VERSION,
} from "./contracts";

describe("probe contract", () => {
  it("pins the M0 contract version", () => {
    expect(PROBE_VERSION).toBe("m0");
  });

  it("accepts the exact v2 preferences contract", () => {
    expect(isProbePreferences({
      schemaVersion: PREFERENCES_SCHEMA_VERSION,
      x: -120,
      y: 80,
      width: 420,
      height: 520,
      displayScale: 1.25,
      flipped: true,
      mode: "desktop",
    })).toBe(true);
  });

  it.each([
    null,
    [],
    {},
    { schemaVersion: 3, x: 0, y: 0, width: 420, height: 520, displayScale: 1, flipped: false, mode: "companion" },
    { schemaVersion: 2, x: 0.5, y: 0, width: 420, height: 520, displayScale: 1, flipped: false, mode: "companion" },
    { schemaVersion: 2, x: 0, y: 0, width: 0, height: 520, displayScale: 1, flipped: false, mode: "companion" },
    { schemaVersion: 2, x: 0, y: 0, width: 420, height: 520, displayScale: 0.49, flipped: false, mode: "companion" },
    { schemaVersion: 2, x: 0, y: 0, width: 420, height: 520, displayScale: 1.51, flipped: false, mode: "companion" },
    { schemaVersion: 2, x: 0, y: 0, width: 420, height: 520, displayScale: Number.NaN, flipped: false, mode: "companion" },
    { schemaVersion: 2, x: 0, y: 0, width: 420, height: 520, displayScale: 1, flipped: "false", mode: "companion" },
    { schemaVersion: 2, x: 0, y: 0, width: 420, height: 520, displayScale: 1, flipped: false, mode: "floating" },
  ])("rejects invalid preferences input %#", (value) => {
    expect(isProbePreferences(value)).toBe(false);
  });
});
