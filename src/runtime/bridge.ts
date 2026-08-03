import { invoke } from "@tauri-apps/api/core";
import type { HitRegionPayload } from "./contracts";

export interface HitRegionEvidence {
  spanCount: number;
  applied: boolean;
  strategy: string;
  scaleFactor: number;
}

export function applyHitRegion(payload: HitRegionPayload): Promise<HitRegionEvidence> {
  return invoke("apply_hit_region", { payload });
}
