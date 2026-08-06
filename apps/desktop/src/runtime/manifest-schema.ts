export const MANIFEST_SCHEMA_VERSION = 1 as const;

export interface ManifestFileEntry {
  role: string;
  relativePath: string;
  sha256: string;
}

export interface ManifestVec2 {
  x: number;
  y: number;
}

/**
 * Part-level rig contract (foundation for parts-based / skeleton runtime).
 * `anchor` and `pivot` are normalized 0..1 coordinates inside the part
 * texture; `zIndex` is the draw order; `boneId` links the part to a skeleton
 * bone defined at runtime (optional for root-attached parts).
 */
export interface ManifestPart {
  role: string;
  relativePath: string;
  anchor: ManifestVec2;
  pivot: ManifestVec2;
  zIndex: number;
  deformable: boolean;
  boneId?: string;
}

export interface ManifestFeatureBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Normalized (0..1 relative to the body image) feature regions used by the
 * single-image mesh rig. Produced by the vision landmark analysis so the
 * runtime does not have to guess where eyes/ears/tail are.
 */
export interface ManifestMeshFeatures {
  leftEye: ManifestFeatureBox;
  rightEye: ManifestFeatureBox;
  leftEar: ManifestFeatureBox;
  rightEar: ManifestFeatureBox;
  tail: ManifestFeatureBox;
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
  parts?: ManifestPart[];
  meshFeatures?: ManifestMeshFeatures;
}

const SHA256_HEX = /^[0-9a-f]{64}$/;

function isNormalizedVec2(value: unknown): value is ManifestVec2 {
  if (typeof value !== "object" || value === null) return false;
  const vec = value as Record<string, unknown>;
  return (
    typeof vec.x === "number" && Number.isFinite(vec.x) && vec.x >= 0 && vec.x <= 1
    && typeof vec.y === "number" && Number.isFinite(vec.y) && vec.y >= 0 && vec.y <= 1
  );
}

function isNormalizedBox(value: unknown): value is ManifestFeatureBox {
  if (typeof value !== "object" || value === null) return false;
  const box = value as Record<string, unknown>;
  return (
    typeof box.x === "number" && Number.isFinite(box.x) && box.x >= 0 && box.x <= 1
    && typeof box.y === "number" && Number.isFinite(box.y) && box.y >= 0 && box.y <= 1
    && typeof box.width === "number" && Number.isFinite(box.width) && box.width >= 0 && box.width <= 1
    && typeof box.height === "number" && Number.isFinite(box.height) && box.height >= 0 && box.height <= 1
    && box.x + box.width <= 1.001
    && box.y + box.height <= 1.001
  );
}

function parseMeshFeatures(value: unknown): ManifestMeshFeatures | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "object" || value === null) {
    throw new Error("invalid meshFeatures block");
  }
  const mesh = value as Record<string, unknown>;
  for (const key of ["leftEye", "rightEye", "leftEar", "rightEar", "tail"] as const) {
    if (!isNormalizedBox(mesh[key])) {
      throw new Error(`invalid meshFeatures.${key}: must be a normalized box`);
    }
  }
  return mesh as unknown as ManifestMeshFeatures;
}

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
  }
  const animation = value.animation as Record<string, unknown>;
  if (
    typeof animation?.idleFps !== "number"
    || typeof animation?.blinkMsMin !== "number"
    || typeof animation?.blinkMsMax !== "number"
  ) {
    throw new Error("invalid animation block");
  }
  if (value.parts !== undefined) {
    if (!Array.isArray(value.parts) || value.parts.length === 0) {
      throw new Error("parts must declare at least one part");
    }
    const seenRoles = new Set<string>();
    for (const entry of value.parts) {
      const part = entry as Record<string, unknown>;
      if (
        typeof part.role !== "string" || part.role.length === 0
        || typeof part.relativePath !== "string" || part.relativePath.length === 0
        || !isNormalizedVec2(part.anchor)
        || !isNormalizedVec2(part.pivot)
        || typeof part.zIndex !== "number" || !Number.isInteger(part.zIndex)
        || typeof part.deformable !== "boolean"
        || (part.boneId != null && (typeof part.boneId !== "string" || part.boneId.length === 0))
      ) {
        throw new Error("invalid part entry: role/relativePath/anchor/pivot/zIndex/deformable/boneId");
      }
      if (seenRoles.has(part.role)) {
        throw new Error(`duplicate part role: ${part.role}`);
      }
      seenRoles.add(part.role);
    }
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
    parts: value.parts === undefined
      ? undefined
      : (value.parts as Array<Record<string, unknown>>).map((part) => ({
        role: part.role as string,
        relativePath: part.relativePath as string,
        anchor: part.anchor as ManifestVec2,
        pivot: part.pivot as ManifestVec2,
        zIndex: part.zIndex as number,
        deformable: part.deformable as boolean,
        boneId: part.boneId == null ? undefined : (part.boneId as string),
      })),
    meshFeatures: parseMeshFeatures(value.meshFeatures),
  };
}
