import { normalizeAssetPath } from "../runtime-assets/live2d-manifest";
import type { ManifestFileEntry } from "./manifest-schema";

export interface NormalizedRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface MotionProfileV1 {
  profileVersion: 1;
  engineProfile: "life-v1";
  alphaBounds: NormalizedRect;
  breathZone: NormalizedRect;
  swayPivot: { x: number; y: number };
}

export interface RuntimeAssetManifestV3 {
  schemaVersion: 3;
  renderer: "animated-image-v1";
  petId: string;
  variantId: string;
  image: string;
  motionProfile: string;
  files: ManifestFileEntry[];
}

const SHA256_HEX = /^[0-9a-f]{64}$/i;

function object(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${name} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requiredString(value: Record<string, unknown>, field: string): string {
  if (typeof value[field] !== "string" || value[field].length === 0) {
    throw new Error(`missing or invalid ${field}`);
  }
  return value[field] as string;
}

function parseRect(value: unknown, name: string): NormalizedRect {
  const rect = object(value, name);
  const values = [rect.left, rect.top, rect.right, rect.bottom];
  for (const number of values) {
    if (typeof number !== "number" || !Number.isFinite(number)) {
      throw new Error(`${name} contains a non-finite value`);
    }
    if (number < 0 || number > 1) throw new Error(`${name} is out of range`);
  }
  const [left, top, right, bottom] = values as [number, number, number, number];
  if (left >= right || top >= bottom) throw new Error(`${name} is an inverted rect`);
  return { left, top, right, bottom };
}

function parsePoint(value: unknown, name: string): { x: number; y: number } {
  const point = object(value, name);
  const parsed = { x: point.x, y: point.y };
  for (const number of Object.values(parsed)) {
    if (typeof number !== "number" || !Number.isFinite(number)) {
      throw new Error(`${name} contains a non-finite value`);
    }
    if (number < 0 || number > 1) throw new Error(`${name} is out of range`);
  }
  return parsed as { x: number; y: number };
}

export function parseMotionProfile(json: unknown): MotionProfileV1 {
  const value = object(json, "motion profile");
  if (value.profileVersion !== 1) throw new Error("unknown profile version");
  if (value.engineProfile !== "life-v1") throw new Error("unknown engine profile");
  const alphaBounds = parseRect(value.alphaBounds, "alpha bounds");
  const breathZone = parseRect(value.breathZone, "breath zone");
  const swayPivot = parsePoint(value.swayPivot, "sway pivot");

  if (
    breathZone.left < alphaBounds.left
    || breathZone.top < alphaBounds.top
    || breathZone.right > alphaBounds.right
    || breathZone.bottom > alphaBounds.bottom
  ) {
    throw new Error("breath zone is outside alpha bounds");
  }
  const faceSafetyLine = alphaBounds.top + (alphaBounds.bottom - alphaBounds.top) * 0.4;
  if (breathZone.top < faceSafetyLine) throw new Error("breath zone violates face safety line");
  if (
    swayPivot.x < alphaBounds.left
    || swayPivot.x > alphaBounds.right
    || swayPivot.y < alphaBounds.top
    || swayPivot.y > alphaBounds.bottom
  ) {
    throw new Error("sway pivot is outside alpha bounds");
  }

  return { profileVersion: 1, engineProfile: "life-v1", alphaBounds, breathZone, swayPivot };
}

function parseFileEntries(value: unknown): ManifestFileEntry[] {
  if (!Array.isArray(value) || value.length === 0) throw new Error("manifest must declare files");
  const seenPaths = new Set<string>();
  return value.map((entry) => {
    const file = object(entry, "file entry");
    const role = requiredString(file, "role");
    const relativePath = normalizeAssetPath(requiredString(file, "relativePath"));
    const sha256 = requiredString(file, "sha256");
    if (!SHA256_HEX.test(sha256)) throw new Error("invalid file entry: sha256 must be 64 hex chars");
    if (seenPaths.has(relativePath)) throw new Error(`duplicate asset path: ${relativePath}`);
    seenPaths.add(relativePath);
    if (!relativePath.toLowerCase().endsWith(".png") && !relativePath.toLowerCase().endsWith(".json")) {
      throw new Error(`unsupported asset extension: ${relativePath}`);
    }
    return { role, relativePath, sha256: sha256.toLowerCase() };
  });
}

function requiredRelativePath(value: Record<string, unknown>, field: string): string {
  try {
    return normalizeAssetPath(requiredString(value, field));
  } catch {
    throw new Error(`${field} must be a relative path`);
  }
}

export function parseAnimatedImageManifest(json: unknown): RuntimeAssetManifestV3 {
  const value = object(json, "manifest");
  if (value.schemaVersion !== 3) throw new Error(`unsupported schemaVersion: ${String(value.schemaVersion)}`);
  if (value.renderer !== "animated-image-v1") throw new Error(`unsupported renderer: ${String(value.renderer)}`);
  const image = requiredRelativePath(value, "image");
  const motionProfile = requiredRelativePath(value, "motionProfile");
  if (!image.toLowerCase().endsWith(".png")) throw new Error("image must be a PNG file");
  if (!motionProfile.toLowerCase().endsWith(".json")) throw new Error("motionProfile must be a JSON file");
  const files = parseFileEntries(value.files);
  if (!files.some((file) => file.role === "main" && file.relativePath === image)) {
    throw new Error("image is not listed as the main file");
  }
  if (!files.some((file) => file.role === "motion-profile" && file.relativePath === motionProfile)) {
    throw new Error("motionProfile is not listed as the motion-profile file");
  }
  return {
    schemaVersion: 3,
    renderer: "animated-image-v1",
    petId: requiredString(value, "petId"),
    variantId: requiredString(value, "variantId"),
    image,
    motionProfile,
    files,
  };
}
