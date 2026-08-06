import { describe, expect, it } from "vitest";
import { blendParams, defaultParams, mergeParams, PARAM_KEYS } from "./params";

describe("params", () => {
  it("defaults every channel to its neutral value", () => {
    const params = defaultParams();
    for (const key of PARAM_KEYS) {
      expect(params[key]).toBe(key === "squash" ? 1 : 0);
    }
  });

  it("merges a patch into a base copy", () => {
    const params = mergeParams(defaultParams(), { tilt: 6, accent: 1 });
    expect(params.tilt).toBe(6);
    expect(params.accent).toBe(1);
    expect(params.breath).toBe(0);
  });

  it("blends between two parameter sets", () => {
    const a = mergeParams(defaultParams(), { tilt: 0 });
    const b = mergeParams(defaultParams(), { tilt: 10 });
    expect(blendParams(a, b, 0.5).tilt).toBeCloseTo(5, 3);
  });

  it("clamps the blend weight to 0..1", () => {
    const a = mergeParams(defaultParams(), { tilt: 0 });
    const b = mergeParams(defaultParams(), { tilt: 10 });
    expect(blendParams(a, b, -1).tilt).toBe(0);
    expect(blendParams(a, b, 2).tilt).toBe(10);
  });
});
