import type { PetRenderAsset } from "./pet-renderer";
import {
  parseFrameSequenceManifest,
  type RuntimeAssetManifestV7,
} from "./frame-sequence-manifest";

export type FrameSequenceRenderAsset = Extract<PetRenderAsset, { kind: "frame-sequence" }>;

export async function loadFrameSequenceAsset(
  petId: string,
  manifest: RuntimeAssetManifestV7,
  assetUrl: (petId: string, path: string) => string,
): Promise<FrameSequenceRenderAsset> {
  const parsed = parseFrameSequenceManifest(manifest);
  if (parsed.petId !== petId) throw new Error("manifest mismatch");
  return {
    kind: "frame-sequence",
    baseImageUrl: assetUrl(petId, parsed.baseImage),
    actions: parsed.actions.map((action) => ({
      actionId: action.actionId,
      loop: action.loop,
      frameDurationMs: action.frameDurationMs,
      frameUrls: action.frames.map((frame) => assetUrl(petId, frame)),
      holdRange: action.holdRange ? [action.holdRange[0], action.holdRange[1]] : undefined,
    })),
    defaultAction: parsed.defaultAction,
    semantics: { ...parsed.semantics },
    idleSchedule: parsed.idleSchedule
      ? {
          entries: parsed.idleSchedule.entries.map((e) => ({ ...e })),
          minIntervalMs: parsed.idleSchedule.minIntervalMs,
          maxIntervalMs: parsed.idleSchedule.maxIntervalMs,
          alignToDefaultLoop: parsed.idleSchedule.alignToDefaultLoop,
        }
      : null,
    hitBounds: parsed.hitBounds,
  };
}
