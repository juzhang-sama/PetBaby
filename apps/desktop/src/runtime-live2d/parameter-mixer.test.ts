import { describe, expect, it, vi } from "vitest";
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
});
