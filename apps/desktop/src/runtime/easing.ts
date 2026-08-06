export type Easing = (t: number) => number;

export type EasingName = "linear" | "easeOutCubic" | "easeInOutQuad";

export const linear: Easing = (t) => t;

export const easeOutCubic: Easing = (t) => 1 - Math.pow(1 - t, 3);

export const easeInOutQuad: Easing = (t) => (
  t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2
);

const BY_NAME: Record<EasingName, Easing> = {
  linear,
  easeOutCubic,
  easeInOutQuad,
};

export function easeByName(name: EasingName): Easing {
  return BY_NAME[name] ?? linear;
}
