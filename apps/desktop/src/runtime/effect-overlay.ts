import type { PetEffect } from "./pet-presentation-controller";

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

  play(effect: PetEffect): void {
    if (this.destroyed) return;
    this.clear();
    const particles = Array.from({ length: PARTICLE_COUNT }, (_, index) => {
      const particle = this.createElement();
      particle.className = "pet-effect-particle";
      particle.dataset.shape = effect;
      particle.textContent = effect === "hearts" ? "♥" : effect === "sparkles" ? "+" : "_";
      particle.style.setProperty("--particle-index", String(index));
      particle.style.setProperty("--particle-x", `${31 + index * 6.2}%`);
      particle.style.setProperty("--particle-y", `${54 - index % 3 * 7}%`);
      particle.style.setProperty("--particle-drift", `${(index - 3) * 9}px`);
      return particle;
    });
    this.layer.dataset.effect = effect;
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
    this.layer.hidden = true;
    this.layer.replaceChildren();
  }
}
