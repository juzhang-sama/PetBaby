import { invoke } from "@tauri-apps/api/core";
import type { HitRegionPayload, ProbePreferences } from "./contracts";

export interface HitRegionEvidence {
  spanCount: number;
  applied: boolean;
  strategy: string;
  scaleFactor: number;
}

export function applyHitRegion(payload: HitRegionPayload): Promise<HitRegionEvidence> {
  return invoke("apply_hit_region", { payload });
}

export const loadPreferences = (): Promise<ProbePreferences> => invoke("load_preferences");
export const savePreferences = (value: ProbePreferences): Promise<void> => invoke("save_preferences", { value });
export const beginDrag = (): Promise<void> => invoke("begin_drag");
