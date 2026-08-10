import type { Live2DSemantics } from "../runtime/pet-renderer";

export const LIVE2D_MANIFEST_SCHEMA_VERSION = 2 as const;
const SHA256_HEX = /^[0-9a-f]{64}$/i;
const ALLOWED_EXTENSIONS = new Set([".json", ".moc3", ".png", ".motion3.json", ".exp3.json", ".physics3.json", ".pose3.json", ".userdata3.json"]);

export interface Live2DManifestFile {
  role: string;
  relativePath: string;
  sha256: string;
}

export interface Live2DLicense {
  id: string;
  author: string;
  source: string;
  commercialUse: boolean;
  redistributable: boolean;
}

export interface RuntimeAssetManifestV2 {
  schemaVersion: 2;
  renderer: "live2d-v1";
  petId: string;
  variantId: string;
  modelEntry: string;
  previewImage: string;
  files: Live2DManifestFile[];
  semantics: Live2DSemantics;
  license: Live2DLicense;
}

export function normalizeAssetPath(input: string): string {
  if (typeof input !== "string" || input.length === 0) throw new Error("asset path must not be empty");
  const normalized = input.replaceAll("\\", "/");
  if (normalized.startsWith("/") || normalized.includes(":")) throw new Error(`absolute asset path is not allowed: ${input}`);
  const parts = normalized.split("/");
  if (parts.some((part) => part.length === 0 || part === "." || part === "..")) throw new Error(`unsafe asset path: ${input}`);
  return parts.join("/");
}

function validateExtension(path: string): void {
  const lower = path.toLowerCase();
  if (![...ALLOWED_EXTENSIONS].some((extension) => lower.endsWith(extension))) throw new Error(`unsupported asset extension: ${path}`);
}

function requiredString(value: Record<string, unknown>, field: string): string {
  if (typeof value[field] !== "string" || value[field].length === 0) throw new Error(`missing or invalid ${field}`);
  return value[field] as string;
}

export function parseLive2DManifest(json: unknown): RuntimeAssetManifestV2 {
  if (typeof json !== "object" || json === null) throw new Error("manifest must be an object");
  const value = json as Record<string, unknown>;
  if (value.schemaVersion !== 2) throw new Error(`unsupported schemaVersion: ${String(value.schemaVersion)}`);
  if (value.renderer !== "live2d-v1") throw new Error(`unsupported renderer: ${String(value.renderer)}`);
  const modelEntry = normalizeAssetPath(requiredString(value, "modelEntry"));
  const previewImage = normalizeAssetPath(requiredString(value, "previewImage"));
  validateExtension(modelEntry);
  validateExtension(previewImage);
  const filesValue = value.files;
  if (!Array.isArray(filesValue) || filesValue.length === 0) throw new Error("manifest must declare files");
  const seenPaths = new Set<string>();
  const files = filesValue.map((entry) => {
    if (typeof entry !== "object" || entry === null) throw new Error("invalid file entry");
    const file = entry as Record<string, unknown>;
    const role = requiredString(file, "role");
    const relativePath = normalizeAssetPath(requiredString(file, "relativePath"));
    const sha256 = requiredString(file, "sha256");
    validateExtension(relativePath);
    if (!SHA256_HEX.test(sha256)) throw new Error("invalid file entry: sha256 must be 64 hex chars");
    if (seenPaths.has(relativePath)) throw new Error(`duplicate asset path: ${relativePath}`);
    seenPaths.add(relativePath);
    return { role, relativePath, sha256: sha256.toLowerCase() };
  });
  if (!files.some((file) => file.relativePath === modelEntry)) throw new Error("modelEntry is not listed in files");
  if (!files.some((file) => file.relativePath === previewImage)) throw new Error("previewImage is not listed in files");
  if (typeof value.semantics !== "object" || value.semantics === null) throw new Error("missing or invalid semantics");
  const semantics = value.semantics as Record<string, unknown>;
  for (const key of ["motions", "expressions", "hitAreas", "parameters"]) if (typeof semantics[key] !== "object" || semantics[key] === null) throw new Error(`invalid semantics.${key}`);
  const known = { motions: new Set(["idle", "look-left", "look-right", "react-happy", "react-curious", "sleep", "wake", "carried", "landed"]), expressions: new Set(["neutral", "happy", "curious", "sleepy", "sad", "angry"]), hitAreas: new Set(["head", "body"]), parameters: new Set(["eyeOpen", "eyeBallX", "eyeBallY", "angleX", "angleY", "bodyBreath", "bodySway", "mouthOpen"]) };
  for (const [group, allowed] of Object.entries(known)) for (const key of Object.keys((semantics[group] as object))) if (!allowed.has(key)) throw new Error(`unknown semantics.${group}.${key}`);
  for (const [key, mapping] of Object.entries(semantics.motions as Record<string, unknown>)) {
    const motion = mapping as Record<string, unknown>;
    if (
      typeof mapping !== "object"
      || mapping === null
      || typeof motion.group !== "string"
      || motion.group.length === 0
      || (motion.index !== undefined && (!Number.isInteger(motion.index) || (motion.index as number) < 0))
    ) throw new Error(`invalid semantics.motions.${key}`);
  }
  for (const group of ["expressions", "hitAreas", "parameters"] as const) for (const [key, mapping] of Object.entries(semantics[group] as Record<string, unknown>)) if (typeof mapping !== "string" || mapping.length === 0) throw new Error(`invalid semantics.${group}.${key}`);
  const license = value.license;
  if (typeof license !== "object" || license === null) throw new Error("missing or invalid license");
  const licenseValue = license as Record<string, unknown>;
  const id = requiredString(licenseValue, "id");
  const author = requiredString(licenseValue, "author");
  const source = requiredString(licenseValue, "source");
  if (typeof licenseValue.commercialUse !== "boolean") throw new Error("missing or invalid commercialUse");
  if (typeof licenseValue.redistributable !== "boolean") throw new Error("missing or invalid redistributable");
  return { schemaVersion: 2, renderer: "live2d-v1", petId: requiredString(value, "petId"), variantId: requiredString(value, "variantId"), modelEntry, previewImage, files, semantics: value.semantics as Live2DSemantics, license: { id, author, source, commercialUse: licenseValue.commercialUse, redistributable: licenseValue.redistributable } };
}
