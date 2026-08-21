import {
  CAT_CHARACTER_RENDERER,
  CAT_SKELETON_VERSION,
  parseCatCharacterManifest,
  type RuntimeAssetManifestV4,
} from "./cat-character-manifest";
import { normalizeAssetPath } from "./live2d-manifest";
import { BODY_MODULE_IDS_V1, type BodyModuleIdV1 } from "./cat-motion-spatial-profile";

export const CAT_SPATIAL_MANIFEST_SCHEMA_VERSION = 5 as const;
export const CAT_SPATIAL_CHARACTER_RENDERER = "cat-spatial-live2d-v1" as const;

export type RuntimeAssetManifestV5 = Omit<
  RuntimeAssetManifestV4,
  "schemaVersion" | "renderer"
> & {
  schemaVersion: 5;
  renderer: "cat-spatial-live2d-v1";
  bodyModuleId: BodyModuleIdV1;
  motionSpatialProfile: string;
};

const BODY_MODULE_IDS = new Set<string>(BODY_MODULE_IDS_V1);

function requiredString(value: Record<string, unknown>, field: string): string {
  const result = value[field];
  if (typeof result !== "string" || result.length === 0) {
    throw new Error(`missing or invalid ${field}`);
  }
  return result;
}

export function parseCatSpatialManifest(json: unknown): RuntimeAssetManifestV5 {
  if (typeof json !== "object" || json === null || Array.isArray(json)) {
    throw new Error("manifest must be an object");
  }
  const value = json as Record<string, unknown>;
  if (value.schemaVersion !== CAT_SPATIAL_MANIFEST_SCHEMA_VERSION) {
    throw new Error(`unsupported schemaVersion: ${String(value.schemaVersion)}`);
  }
  if (value.renderer !== CAT_SPATIAL_CHARACTER_RENDERER) {
    throw new Error(`unsupported renderer: ${String(value.renderer)}`);
  }

  const bodyModuleId = requiredString(value, "bodyModuleId");
  if (!BODY_MODULE_IDS.has(bodyModuleId)) {
    throw new Error("bodyModuleId is not a supported body module");
  }
  const motionSpatialProfile = normalizeAssetPath(requiredString(value, "motionSpatialProfile"));
  if (!motionSpatialProfile.toLowerCase().endsWith(".json")) {
    throw new Error("motionSpatialProfile must be a JSON file");
  }

  const v4 = parseCatCharacterManifest({
    ...value,
    schemaVersion: 4,
    renderer: CAT_CHARACTER_RENDERER,
  });
  if (!v4.files.some((file) => (
    file.role === "motion-spatial-profile" && file.relativePath === motionSpatialProfile
  ))) {
    throw new Error("motionSpatialProfile is not listed as the motion-spatial-profile file in files");
  }

  return {
    ...v4,
    schemaVersion: CAT_SPATIAL_MANIFEST_SCHEMA_VERSION,
    renderer: CAT_SPATIAL_CHARACTER_RENDERER,
    skeletonVersion: CAT_SKELETON_VERSION,
    bodyModuleId: bodyModuleId as BodyModuleIdV1,
    motionSpatialProfile,
  };
}

export function buildCatSpatialManifest(input: unknown): RuntimeAssetManifestV5 {
  return parseCatSpatialManifest(input);
}
