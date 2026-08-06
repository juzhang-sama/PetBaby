export const PARAM_KEYS = [
  "breath",
  "blinkLeft",
  "blinkRight",
  "lookX",
  "lookY",
  "earLeft",
  "earRight",
  "tail",
  "squash",
  "shiftX",
  "shiftY",
  "tilt",
  "accent",
] as const;

export type ParamKey = (typeof PARAM_KEYS)[number];

export type Params = Record<ParamKey, number>;

export function defaultParams(): Params {
  return {
    breath: 0,
    blinkLeft: 0,
    blinkRight: 0,
    lookX: 0,
    lookY: 0,
    earLeft: 0,
    earRight: 0,
    tail: 0,
    squash: 1,
    shiftX: 0,
    shiftY: 0,
    tilt: 0,
    accent: 0,
  };
}

export function mergeParams(base: Params, patch: Partial<Params>): Params {
  return { ...base, ...patch };
}

export function blendParams(a: Params, b: Params, weight: number): Params {
  const t = Math.max(0, Math.min(1, weight));
  const out = {} as Params;
  for (const key of PARAM_KEYS) {
    out[key] = a[key] + (b[key] - a[key]) * t;
  }
  return out;
}
