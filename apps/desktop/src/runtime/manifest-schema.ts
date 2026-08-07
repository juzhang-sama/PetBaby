export const MANIFEST_SCHEMA_VERSION = 1 as const;
import { parseLive2DManifest, type RuntimeAssetManifestV2 } from "../runtime-assets/live2d-manifest";

export interface ManifestFileEntry {
  role: string;
  relativePath: string;
  sha256: string;
}

export interface RuntimeAssetManifestV1 {
  schemaVersion: 1;
  assetType: "single-image" | "layered-v1";
  petId: string;
  variantId: string;
  styleId: "signature-cartoon-v1";
  view: "front";
  pose: "sitting";
  files: ManifestFileEntry[];
  animation: { idleFps: number; blinkMsMin: number; blinkMsMax: number };
}

const SHA256_HEX = /^[0-9a-f]{64}$/;

export function parseManifestV1(json: unknown): RuntimeAssetManifestV1 {
  if (typeof json !== "object" || json === null) {
    throw new Error("manifest must be an object");
  }
  const value = json as Record<string, unknown>;

  if (value.schemaVersion !== MANIFEST_SCHEMA_VERSION) {
    throw new Error(`unsupported schemaVersion: ${String(value.schemaVersion)}`);
  }
  if (value.assetType !== "single-image" && value.assetType !== "layered-v1") {
    throw new Error(`unsupported assetType: ${String(value.assetType)}`);
  }
  for (const field of ["petId", "variantId", "styleId", "view", "pose"] as const) {
    if (typeof value[field] !== "string" || (value[field] as string).length === 0) {
      throw new Error(`missing or invalid ${field}`);
    }
  }
  if (!Array.isArray(value.files) || value.files.length === 0) {
    throw new Error("manifest must declare at least one file");
  }
  for (const entry of value.files) {
    const file = entry as Record<string, unknown>;
    if (
      typeof file.role !== "string"
      || typeof file.relativePath !== "string"
      || typeof file.sha256 !== "string"
      || !SHA256_HEX.test(file.sha256)
    ) {
      throw new Error("invalid file entry: sha256 must be 64 hex chars");
    }
    if (!file.relativePath.toLowerCase().endsWith(".png")) throw new Error("v1 manifests only support PNG fallback assets");
  }
  const animation = value.animation as Record<string, unknown>;
  if (
    typeof animation?.idleFps !== "number"
    || typeof animation?.blinkMsMin !== "number"
    || typeof animation?.blinkMsMax !== "number"
  ) {
    throw new Error("invalid animation block");
  }

  return {
    schemaVersion: MANIFEST_SCHEMA_VERSION,
    assetType: value.assetType,
    petId: value.petId as string,
    variantId: value.variantId as string,
    styleId: value.styleId as "signature-cartoon-v1",
    view: value.view as "front",
    pose: value.pose as "sitting",
    files: value.files as ManifestFileEntry[],
    animation: {
      idleFps: animation.idleFps as number,
      blinkMsMin: animation.blinkMsMin as number,
      blinkMsMax: animation.blinkMsMax as number,
    },
  };
}

export function parseManifest(json: unknown): RuntimeAssetManifestV1 | RuntimeAssetManifestV2 {
  if (typeof json !== "object" || json === null) throw new Error("manifest must be an object");
  return (json as Record<string, unknown>).schemaVersion === 2 ? parseLive2DManifest(json) : parseManifestV1(json);
}
