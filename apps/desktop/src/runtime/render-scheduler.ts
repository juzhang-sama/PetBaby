import type { RenderTier } from "./contracts";

export interface RenderDriver {
  start(): void;
  stop(): void;
  setMaxFps(fps: number): void;
  renderOnce(): void;
}

export class RenderScheduler {
  private companionFps = 24;
  private currentTier: RenderTier | null = null;
  private appliedFps: number | null = null;

  constructor(private readonly driver: RenderDriver) {}

  setCompanionFps(fps: 24 | 60): void {
    this.companionFps = fps;
    if (this.currentTier === "companion") this.applyFps(fps);
  }

  setTier(tier: RenderTier): void {
    this.currentTier = tier;
    if (tier === "active") {
      this.applyFps(60);
      this.driver.start();
      return;
    }
    if (tier === "companion") {
      this.applyFps(this.companionFps);
      this.driver.start();
      return;
    }
    this.driver.stop();
    if (tier === "still") this.driver.renderOnce();
  }

  private applyFps(fps: number): void {
    if (this.appliedFps === fps) return;
    this.appliedFps = fps;
    this.driver.setMaxFps(fps);
  }
}
