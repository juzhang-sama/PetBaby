import type { PetEffect, PetEffectVisualOptions } from "./pet-presentation-controller";

export interface EffectOverlayOptions {
  createElement?: () => HTMLElement;
  setTimer?: (callback: () => void, delayMs: number) => number;
  clearTimer?: (timer: number) => void;
}

const PARTICLE_COUNT = 7;

export class EffectOverlay {
  private readonly layer: HTMLElement;
  private readonly createElement: () => HTMLElement;
  private readonly setTimer: (callback: () => void, delayMs: number) => number;
  private readonly clearTimer: (timer: number) => void;
  private timer: number | null = null;
  private destroyed = false;

  constructor(root: HTMLElement, options: EffectOverlayOptions = {}) {
    this.createElement = options.createElement ?? (() => document.createElement("span"));
    this.setTimer = options.setTimer ?? ((callback, delayMs) => window.setTimeout(callback, delayMs));
    this.clearTimer = options.clearTimer ?? ((timer) => window.clearTimeout(timer));
    this.layer = this.createElement();
    this.layer.className = "pet-effect-overlay";
    this.layer.hidden = true;
    root.append(this.layer);
  }

  play(
    effect: PetEffect,
    options: PetEffectVisualOptions = { opacity: 1, intensity: 1 },
  ): void {
    if (this.destroyed) return;
    this.clear();
    const opacity = unitInterval(options.opacity);
    const intensity = unitInterval(options.intensity);
    if (opacity === 0 || intensity === 0) return;
    for (const [name, value] of animationVariables(intensity)) {
      this.layer.style.setProperty(name, value);
    }
    const particles = Array.from({ length: PARTICLE_COUNT }, (_, index) => {
      const particle = this.createElement();
      particle.className = "pet-effect-particle";
      particle.dataset.shape = effect;
      particle.textContent = effect === "hearts" ? "♥" : effect === "sparkles" ? "+" : "_";
      particle.style.setProperty("--particle-index", String(index));
      particle.style.setProperty("--particle-x", `${31 + index * 6.2}%`);
      particle.style.setProperty("--particle-y", `${54 - index % 3 * 7}%`);
      particle.style.setProperty(
        "--particle-drift",
        `${formatNumber((index - 3) * 9 * intensity)}px`,
      );
      return particle;
    });
    this.layer.dataset.effect = effect;
    this.layer.style.opacity = String(opacity);
    this.layer.hidden = false;
    this.layer.replaceChildren(...particles);
    this.timer = this.setTimer(() => this.clear(), 700);
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.clear();
    this.layer.remove();
  }

  private clear(): void {
    if (this.timer !== null) {
      this.clearTimer(this.timer);
      this.timer = null;
    }
    delete this.layer.dataset.effect;
    this.layer.style.opacity = "";
    this.layer.hidden = true;
    this.layer.replaceChildren();
  }
}

function unitInterval(value: number): number {
  return Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 1;
}

function animationVariables(intensity: number): ReadonlyArray<readonly [string, string]> {
  return [
    ["--pet-effect-rise-start-y", `${formatNumber(12 * intensity)}px`],
    ["--pet-effect-rise-end-y", `${formatNumber(-54 * intensity)}px`],
    ["--pet-effect-heart-start-scale", formatNumber(scaleFromNeutral(0.65, intensity))],
    ["--pet-effect-heart-end-scale", formatNumber(scaleFromNeutral(1.08, intensity))],
    ["--pet-effect-spark-end-rotation", `${formatNumber(90 * intensity)}deg`],
    ["--pet-effect-spark-start-scale", formatNumber(scaleFromNeutral(0.4, intensity))],
    ["--pet-effect-spark-end-scale", formatNumber(scaleFromNeutral(1.25, intensity))],
    ["--pet-effect-land-start-scale-x", formatNumber(scaleFromNeutral(0.2, intensity))],
    ["--pet-effect-land-end-scale-x", formatNumber(scaleFromNeutral(1.8, intensity))],
  ];
}

function scaleFromNeutral(fullStrength: number, intensity: number): number {
  return 1 + (fullStrength - 1) * intensity;
}

function formatNumber(value: number): string {
  return String(Math.round(value * 10_000) / 10_000);
}
