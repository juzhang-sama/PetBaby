export const PROBE_VERSION = "m0" as const;

export type WindowMode = "companion" | "desktop";
export type RenderTier = "active" | "companion" | "still" | "paused";

export interface RegionSpan {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface HitRegionPayload {
  canvasWidth: number;
  canvasHeight: number;
  scaleFactor: number;
  spans: RegionSpan[];
}

export interface ProbePreferences {
  x: number; y: number; width: number; height: number;
  scale: number; flipped: boolean; mode: WindowMode;
}
