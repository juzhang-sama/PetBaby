import { describe, expect, it, vi } from "vitest";
import { motionSpatialProfileForTest } from "../runtime-assets/cat-motion-spatial-profile-test-fixtures";
import { ParameterMixer, mixParameters } from "./parameter-mixer";

describe("mixParameters", () => {
  it("mixes lip sync without overwriting eye blink", () => {
    const result = mixParameters({ blink: 0.2, lipSync: 0.8, lookX: 0.4, breath: 0.5 });
    expect(result.eyeOpen).toBeCloseTo(0.2);
    expect(result.mouthOpen).toBeCloseTo(0.8);
    expect(result.eyeBallX).toBeCloseTo(0.4);
    expect(result.bodyBreath).toBeCloseTo(0.5);
  });

  it("applies layers in the documented order", () => {
    const result = mixParameters({
      motion: { angleX: 1 },
      expression: { angleX: 2 },
      automation: { angleX: 3 },
      look: { angleX: 4 },
      lipSyncLayer: { angleX: 5 },
      physics: { angleX: 6 },
    });
    expect(result.angleX).toBe(6);
  });

  it("lets interaction override automation while physics remains last", () => {
    const result = mixParameters({
      automation: { tailAngle: 4, earLeft: 0.2 },
      interaction: { tailAngle: 9, earLeft: -0.3 },
      physics: { tailAngle: 7 },
    });

    expect(result.tailAngle).toBe(7);
    expect(result.earLeft).toBe(-0.3);
  });

  it("mixes body sway independently from breath", () => {
    const result = mixParameters({ breath: 0.8, sway: -4 });

    expect(result).toMatchObject({ bodyBreath: 0.8, bodySway: -4 });
    expect(result.eyeOpen).toBeUndefined();
    expect(result.eyeBallX).toBeUndefined();
    expect(result.angleX).toBeUndefined();
    expect(result.mouthOpen).toBeUndefined();
  });
});

describe("ParameterMixer", () => {
  it("clamps every write to the model parameter range", () => {
    const setParameter = vi.fn();
    const mixer = new ParameterMixer({
      semantics: { mouthOpen: "ParamMouthOpenY" },
      port: {
        getParameterRange: () => ({ min: 0, max: 1 }),
        setParameter,
      },
    });

    mixer.apply({ lipSync: 2 });

    expect(setParameter).toHaveBeenCalledWith("ParamMouthOpenY", 1);
  });

  it.each([
    ["body-slender-v1", 0.65],
    ["body-balanced-v1", 0.8],
    ["body-rounded-v1", 0.75],
  ] as const)("clamps breath to the %s approved amplitude", (bodyModuleId, breathMax) => {
    const setParameter = vi.fn();
    const mixer = new ParameterMixer({
      semantics: { bodyBreath: "ParamBreath" },
      motionSpatialProfile: motionSpatialProfileForTest(bodyModuleId, breathMax),
      port: {
        getParameterRange: () => ({ min: 0, max: 1 }),
        setParameter,
      },
    });

    mixer.apply({ breath: 1 });

    expect(setParameter).toHaveBeenCalledWith("ParamBreath", breathMax);
  });

  it("skips a write when the model and character amplitude ranges do not overlap", () => {
    const setParameter = vi.fn();
    const diagnose = vi.fn();
    const mixer = new ParameterMixer({
      semantics: { bodyBreath: "ParamBreath" },
      motionSpatialProfile: motionSpatialProfileForTest(),
      port: {
        getParameterRange: () => ({ min: 2, max: 3 }),
        setParameter,
      },
      diagnose,
    });

    mixer.apply({ breath: 1 });
    mixer.apply({ breath: 0.5 });

    expect(setParameter).not.toHaveBeenCalled();
    expect(diagnose).toHaveBeenCalledOnce();
    expect(diagnose).toHaveBeenCalledWith("bodyBreath", "incompatible-range");
  });

  it("writes the shared boundary when model and character ranges touch", () => {
    const setParameter = vi.fn();
    const diagnose = vi.fn();
    const mixer = new ParameterMixer({
      semantics: { bodyBreath: "ParamBreath" },
      motionSpatialProfile: motionSpatialProfileForTest(),
      port: {
        getParameterRange: () => ({ min: 0.8, max: 2 }),
        setParameter,
      },
      diagnose,
    });

    mixer.apply({ breath: 1 });

    expect(setParameter).toHaveBeenCalledWith("ParamBreath", 0.8);
    expect(diagnose).not.toHaveBeenCalled();
  });

  it("rejects NaN before writing a character amplitude parameter", () => {
    const setParameter = vi.fn();
    const mixer = new ParameterMixer({
      semantics: { bodyBreath: "ParamBreath" },
      motionSpatialProfile: motionSpatialProfileForTest(),
      port: {
        getParameterRange: () => ({ min: 0, max: 1 }),
        setParameter,
      },
    });

    expect(() => mixer.apply({ breath: Number.NaN })).toThrow(/motion value must be finite/i);
    expect(setParameter).not.toHaveBeenCalled();
  });

  it("clamps infinity through the model range before the character amplitude", () => {
    const setParameter = vi.fn();
    const mixer = new ParameterMixer({
      semantics: { bodyBreath: "ParamBreath" },
      motionSpatialProfile: motionSpatialProfileForTest(),
      port: {
        getParameterRange: () => ({ min: 0, max: 1 }),
        setParameter,
      },
    });

    mixer.apply({ breath: Number.POSITIVE_INFINITY });

    expect(setParameter).toHaveBeenCalledWith("ParamBreath", 0.8);
  });

  it("diagnoses each missing semantic only once", () => {
    const diagnose = vi.fn();
    const mixer = new ParameterMixer({
      semantics: {},
      port: { getParameterRange: () => null, setParameter: vi.fn() },
      diagnose,
    });

    mixer.apply({ blink: 0.5, lipSync: 0.2 });
    mixer.apply({ blink: 0.8, lipSync: 0.4 });

    expect(diagnose).toHaveBeenCalledTimes(2);
    expect(diagnose).toHaveBeenCalledWith("eyeOpen");
    expect(diagnose).toHaveBeenCalledWith("mouthOpen");
  });

  it("silently skips optional production micro-motion mappings", () => {
    const diagnose = vi.fn();
    const mixer = new ParameterMixer({
      semantics: {},
      port: { getParameterRange: () => null, setParameter: vi.fn() },
      diagnose,
      silentMissing: new Set(["bodyBreath", "bodySway"]),
    });

    mixer.apply({ breath: 0.8, sway: -4 });

    expect(diagnose).not.toHaveBeenCalled();
  });
});
