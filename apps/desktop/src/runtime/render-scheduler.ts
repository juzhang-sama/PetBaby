import type { RenderTier } from "./contracts";

export interface RenderDriver {
  start(): void;
  stop(): void;
  setMaxFps(fps: number): void;
  renderOnce(): void;
}

export class RenderScheduler {
  constructor(private readonly driver: RenderDriver) {}

  setTier(tier: RenderTier): void {
    if (tier === "active") {
      this.driver.setMaxFps(60);
      this.driver.start();
      return;
    }
    if (tier === "companion") {
      this.driver.setMaxFps(24);
      this.driver.start();
      return;
    }
    this.driver.stop();
    if (tier === "still") this.driver.renderOnce();
  }
}
