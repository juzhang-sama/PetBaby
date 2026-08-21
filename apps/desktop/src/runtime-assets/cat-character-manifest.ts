import {
  CAT_EDGE_TAIL_STATES_V1,
  CAT_HIT_AREAS_V1,
  CAT_MOTION_SET_V1,
  CAT_PARAMETER_SET_V1,
  type CatEdgeTailStateV1,
  type CatHitAreaNameV1,
  type CatMotionNameV1,
  type CatParameterNameV1,
} from "../runtime-live2d/cat-motion-contract";
import {
  normalizeAssetPath,
  type Live2DLicense,
  type Live2DManifestFile,
} from "./live2d-manifest";

export const CAT_CHARACTER_MANIFEST_SCHEMA_VERSION = 4 as const;
export const CAT_CHARACTER_RENDERER = "cat-live2d-v1" as const;
export const CAT_SKELETON_VERSION = "cat-a-live2d-v1" as const;

const SHA256_HEX = /^[0-9a-f]{64}$/i;
const ALLOWED_EXTENSIONS = [
  ".json",
  ".moc3",
  ".png",
  ".motion3.json",
  ".exp3.json",
  ".physics3.json",
  ".pose3.json",
  ".userdata3.json",
] as const;

export interface CatMotionMappingV1 {
  group: string;
  index?: number;
}

export interface CatEdgeTailMappingV1 extends CatMotionMappingV1 {
  tailArtMesh: string;
}

export interface RuntimeAssetManifestV4 {
  schemaVersion: 4;
  renderer: "cat-live2d-v1";
  petId: string;
  variantId: string;
  skeletonVersion: "cat-a-live2d-v1";
  modelEntry: string;
  previewImage: string;
  files: Live2DManifestFile[];
  motions: Record<CatMotionNameV1, CatMotionMappingV1>;
  parameters: Record<CatParameterNameV1, string>;
  hitAreas: Record<CatHitAreaNameV1, string>;
  edgeTailStates: Record<CatEdgeTailStateV1, CatEdgeTailMappingV1>;
  license: Live2DLicense;
}

function requiredString(value: Record<string, unknown>, field: string): string {
  const result = value[field];
  if (typeof result !== "string" || result.length === 0) {
    throw new Error(`missing or invalid ${field}`);
  }
  return result;
}

function requiredObject(value: Record<string, unknown>, field: string): Record<string, unknown> {
  const result = value[field];
  if (typeof result !== "object" || result === null || Array.isArray(result)) {
    throw new Error(`missing or invalid ${field}`);
  }
  return result as Record<string, unknown>;
}

function parseExactRecord<T>(
  value: Record<string, unknown>,
  field: string,
  allowed: readonly string[],
  parseEntry: (entry: unknown, path: string) => T,
): Record<string, T> {
  const source = requiredObject(value, field);
  for (const key of Object.keys(source)) {
    if (!allowed.includes(key)) throw new Error(`unknown ${field}.${key}`);
  }
  const result: Record<string, T> = {};
  for (const key of allowed) {
    if (!(key in source)) throw new Error(`missing ${field}.${key}`);
    result[key] = parseEntry(source[key], `${field}.${key}`);
  }
  return result;
}

function parseMotion(entry: unknown, path: string): CatMotionMappingV1 {
  if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
    throw new Error(`invalid ${path}`);
  }
  const value = entry as Record<string, unknown>;
  const group = requiredString(value, "group");
  const index = value.index;
  if (index !== undefined && (!Number.isInteger(index) || (index as number) < 0)) {
    throw new Error(`invalid ${path}.index`);
  }
  return index === undefined ? { group } : { group, index: index as number };
}

function parseEdgeTail(entry: unknown, path: string): CatEdgeTailMappingV1 {
  if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
    throw new Error(`invalid ${path}`);
  }
  const value = entry as Record<string, unknown>;
  return { ...parseMotion(entry, path), tailArtMesh: requiredString(value, "tailArtMesh") };
}

function parseString(entry: unknown, path: string): string {
  if (typeof entry !== "string" || entry.length === 0) throw new Error(`invalid ${path}`);
  return entry;
}

function validateExtension(path: string): void {
  const lower = path.toLowerCase();
  if (!ALLOWED_EXTENSIONS.some((extension) => lower.endsWith(extension))) {
    throw new Error(`unsupported asset extension: ${path}`);
  }
}

function parseFiles(value: Record<string, unknown>): Live2DManifestFile[] {
  if (!Array.isArray(value.files) || value.files.length === 0) {
    throw new Error("manifest must declare files");
  }
  const seen = new Set<string>();
  return value.files.map((entry) => {
    if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
      throw new Error("invalid file entry");
    }
    const file = entry as Record<string, unknown>;
    const relativePath = normalizeAssetPath(requiredString(file, "relativePath"));
    const key = relativePath.toLowerCase();
    if (seen.has(key)) throw new Error(`duplicate asset path: ${relativePath}`);
    seen.add(key);
    validateExtension(relativePath);
    const sha256 = requiredString(file, "sha256");
    if (!SHA256_HEX.test(sha256)) {
      throw new Error("invalid file entry: sha256 must be 64 hex chars");
    }
    return { role: requiredString(file, "role"), relativePath, sha256: sha256.toLowerCase() };
  });
}

function parseLicense(value: Record<string, unknown>): Live2DLicense {
  const source = requiredObject(value, "license");
  if (typeof source.commercialUse !== "boolean") throw new Error("missing or invalid commercialUse");
  if (source.redistributable !== true) throw new Error("license must be redistributable");
  return {
    id: requiredString(source, "id"),
    author: requiredString(source, "author"),
    source: requiredString(source, "source"),
    commercialUse: source.commercialUse,
    redistributable: true,
  };
}

export function parseCatCharacterManifest(json: unknown): RuntimeAssetManifestV4 {
  if (typeof json !== "object" || json === null || Array.isArray(json)) {
    throw new Error("manifest must be an object");
  }
  const value = json as Record<string, unknown>;
  if (value.schemaVersion !== CAT_CHARACTER_MANIFEST_SCHEMA_VERSION) {
    throw new Error(`unsupported schemaVersion: ${String(value.schemaVersion)}`);
  }
  if (value.renderer !== CAT_CHARACTER_RENDERER) {
    throw new Error(`unsupported renderer: ${String(value.renderer)}`);
  }
  if (value.skeletonVersion !== CAT_SKELETON_VERSION) {
    throw new Error(`unsupported skeletonVersion: ${String(value.skeletonVersion)}`);
  }

  const modelEntry = normalizeAssetPath(requiredString(value, "modelEntry"));
  const previewImage = normalizeAssetPath(requiredString(value, "previewImage"));
  if (!modelEntry.toLowerCase().endsWith(".model3.json")) {
    throw new Error("modelEntry must be a .model3.json file");
  }
  if (!previewImage.toLowerCase().endsWith(".png")) {
    throw new Error("previewImage must be a PNG file");
  }
  const files = parseFiles(value);
  for (const path of [modelEntry, previewImage]) {
    if (!files.some((file) => file.relativePath === path)) {
      throw new Error(`${path} is not listed in files`);
    }
  }

  const motions = parseExactRecord(value, "motions", CAT_MOTION_SET_V1, parseMotion);
  const parameters = parseExactRecord(value, "parameters", CAT_PARAMETER_SET_V1, parseString);
  const hitAreas = parseExactRecord(value, "hitAreas", CAT_HIT_AREAS_V1, parseString);
  const edgeTailStates = parseExactRecord(value, "edgeTailStates", CAT_EDGE_TAIL_STATES_V1, parseEdgeTail);

  if (new Set(Object.values(parameters)).size !== CAT_PARAMETER_SET_V1.length) {
    throw new Error("parameter IDs must be unique so eyes, ears, and tail controls remain independent");
  }
  if (new Set(Object.values(hitAreas)).size !== CAT_HIT_AREAS_V1.length) {
    throw new Error("hit-area IDs must be unique");
  }
  const tailMeshes = new Set(Object.values(edgeTailStates).map((entry) => entry.tailArtMesh));
  if (tailMeshes.size !== 1) {
    throw new Error("all edgeTailStates must reuse the same tail ArtMesh");
  }

  return {
    schemaVersion: 4,
    renderer: CAT_CHARACTER_RENDERER,
    petId: requiredString(value, "petId"),
    variantId: requiredString(value, "variantId"),
    skeletonVersion: CAT_SKELETON_VERSION,
    modelEntry,
    previewImage,
    files,
    motions: motions as RuntimeAssetManifestV4["motions"],
    parameters: parameters as RuntimeAssetManifestV4["parameters"],
    hitAreas: hitAreas as RuntimeAssetManifestV4["hitAreas"],
    edgeTailStates: edgeTailStates as RuntimeAssetManifestV4["edgeTailStates"],
    license: parseLicense(value),
  };
}
