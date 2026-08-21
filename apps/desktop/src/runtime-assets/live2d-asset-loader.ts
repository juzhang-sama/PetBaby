import { invoke } from "@tauri-apps/api/core";
import type { PetRenderAsset } from "../runtime/pet-renderer";
import { parseLive2DManifest, type RuntimeAssetManifestV2 } from "./live2d-manifest";
import { parseCatCharacterManifest, type RuntimeAssetManifestV4 } from "./cat-character-manifest";
import { parseCatSpatialManifest, type RuntimeAssetManifestV5 } from "./cat-spatial-manifest";
import type { Live2DSemantics } from "../runtime/pet-renderer";
import {
  parseMotionSpatialProfileV1,
  type MotionSpatialProfileV1,
} from "./cat-motion-spatial-profile";

export interface Live2DAssetTransport {
  readManifest(petId: string): Promise<unknown>;
  readFile(petId: string, relativePath: string): Promise<Uint8Array>;
}

const defaultTransport: Live2DAssetTransport = {
  readManifest: (petId) => invoke("asset_manifest", { petId }),
  readFile: async (petId, relativePath) => {
    const encoded = await invoke<string>("asset_file_b64", { petId, relativePath });
    const binary = atob(encoded);
    return Uint8Array.from(binary, (char) => char.charCodeAt(0));
  },
};

async function sha256(bytes: Uint8Array): Promise<string> {
  const copy = Uint8Array.from(bytes);
  const digest = await crypto.subtle.digest("SHA-256", copy);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (typeof value === "object" && value !== null) {
    const entries = Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right));
    return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${stableJson(item)}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function mimeType(path: string): string {
  const lower = path.toLowerCase();
  if (lower.endsWith(".png")) return "image/png";
  if (lower.endsWith(".json")) return "application/json";
  return "application/octet-stream";
}

function modelDirectory(path: string): string {
  const separator = path.lastIndexOf("/");
  return separator === -1 ? "" : path.slice(0, separator + 1);
}

function resolveModelReference(modelEntry: string, reference: string): string {
  if (reference.includes(":") || reference.startsWith("/")) {
    throw new Error(`external model reference is not allowed: ${reference}`);
  }
  const resolved: string[] = [];
  for (const segment of `${modelDirectory(modelEntry)}${reference.replaceAll("\\", "/")}`.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (resolved.length === 0) throw new Error(`model reference escapes asset root: ${reference}`);
      resolved.pop();
      continue;
    }
    resolved.push(segment);
  }
  return resolved.join("/");
}

function rewriteModelReferences(
  modelEntry: string,
  modelBytes: Uint8Array,
  urlsByPath: ReadonlyMap<string, string>,
): Uint8Array<ArrayBuffer> {
  let model: unknown;
  try {
    model = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(modelBytes));
  } catch (error) {
    throw new Error(`invalid modelEntry JSON: ${String(error)}`);
  }
  if (typeof model !== "object" || model === null) throw new Error("invalid modelEntry JSON: expected object");
  const root = model as Record<string, unknown>;
  if (typeof root.FileReferences !== "object" || root.FileReferences === null) {
    throw new Error("invalid modelEntry JSON: missing FileReferences");
  }

  const fileKeys = new Set(["Moc", "Physics", "Pose", "UserData", "DisplayInfo", "File", "Sound"]);
  const rewrite = (value: unknown, key?: string): unknown => {
    if (typeof value === "string") {
      if (key !== "Textures" && !fileKeys.has(key ?? "")) return value;
      const path = resolveModelReference(modelEntry, value);
      const url = urlsByPath.get(path);
      if (url !== undefined) return url;
      throw new Error(`model reference is not declared in manifest: ${value}`);
    }
    if (Array.isArray(value)) return value.map((item) => rewrite(item, key));
    if (typeof value === "object" && value !== null) {
      return Object.fromEntries(
        Object.entries(value as Record<string, unknown>).map(([childKey, item]) => [childKey, rewrite(item, childKey)]),
      );
    }
    return value;
  };

  root.FileReferences = rewrite(root.FileReferences);
  return new TextEncoder().encode(JSON.stringify(root));
}

/**
 * The photo-avatar builder writes its v5 manifest with serde_json's field order
 * and two-space pretty formatting. Rebuild that exact representation before
 * acknowledging the backend's manifest-hash CAS.
 */
export async function photoAvatarManifestSha256(manifest: RuntimeAssetManifestV5): Promise<string> {
  const canonical = {
    schemaVersion: manifest.schemaVersion,
    renderer: manifest.renderer,
    petId: manifest.petId,
    variantId: manifest.variantId,
    skeletonVersion: manifest.skeletonVersion,
    bodyModuleId: manifest.bodyModuleId,
    modelEntry: manifest.modelEntry,
    previewImage: manifest.previewImage,
    motionSpatialProfile: manifest.motionSpatialProfile,
    files: manifest.files.map((file) => ({
      role: file.role,
      relativePath: file.relativePath,
      sha256: file.sha256,
    })),
    motions: orderedRecord(manifest.motions, (motion) => ({
      group: motion.group,
      ...(motion.index === undefined ? {} : { index: motion.index }),
    })),
    parameters: orderedRecord(manifest.parameters, (parameter) => parameter),
    hitAreas: orderedRecord(manifest.hitAreas, (hitArea) => hitArea),
    edgeTailStates: orderedRecord(manifest.edgeTailStates, (edge) => ({
      group: edge.group,
      ...(edge.index === undefined ? {} : { index: edge.index }),
      tailArtMesh: edge.tailArtMesh,
    })),
    license: {
      id: manifest.license.id,
      author: manifest.license.author,
      source: manifest.license.source,
      commercialUse: manifest.license.commercialUse,
      redistributable: manifest.license.redistributable,
    },
  };
  return sha256(new TextEncoder().encode(JSON.stringify(canonical, null, 2)));
}

function orderedRecord<T, U>(
  value: Record<string, T>,
  map: (entry: T) => U,
): Record<string, U> {
  return Object.fromEntries(Object.keys(value).sort().map((key) => [key, map(value[key]!)]));
}

function parseVerifiedMotionSpatialProfile(
  manifest: RuntimeAssetManifestV5,
  bytesByPath: ReadonlyMap<string, Uint8Array>,
): MotionSpatialProfileV1 {
  const bytes = bytesByPath.get(manifest.motionSpatialProfile);
  if (bytes === undefined) throw new Error("motionSpatialProfile was not loaded");
  let input: unknown;
  try {
    input = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch (error) {
    throw new Error(`invalid motionSpatialProfile JSON: ${String(error)}`);
  }
  const profile = parseMotionSpatialProfileV1(input);
  if (profile.bodyModuleId !== manifest.bodyModuleId) {
    throw new Error("motionSpatialProfile bodyModuleId does not match manifest");
  }
  return deepFreeze(profile);
}

function deepFreeze<T>(value: T): T {
  if (typeof value === "object" && value !== null && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value as Record<string, unknown>)) deepFreeze(child);
  }
  return value;
}

export async function loadLive2DAsset(
  petId: string,
  expectedManifest: RuntimeAssetManifestV2 | RuntimeAssetManifestV4 | RuntimeAssetManifestV5,
  transport: Live2DAssetTransport = defaultTransport,
): Promise<Extract<PetRenderAsset, { kind: "live2d" }>> {
  const parseManifest = (value: unknown): RuntimeAssetManifestV2 | RuntimeAssetManifestV4 | RuntimeAssetManifestV5 => {
    if (typeof value === "object" && value !== null) {
      const schemaVersion = (value as { schemaVersion?: unknown }).schemaVersion;
      if (schemaVersion === 4) return parseCatCharacterManifest(value);
      if (schemaVersion === 5) return parseCatSpatialManifest(value);
    }
    return parseLive2DManifest(value);
  };
  const expected = parseManifest(expectedManifest);
  if (expected.petId !== petId) throw new Error("manifest mismatch");
  const parsed = parseManifest(await transport.readManifest(petId));
  if (parsed.petId !== petId || stableJson(parsed) !== stableJson(expected)) {
    throw new Error("manifest mismatch");
  }
  const bytesByPath = new Map<string, Uint8Array>();
  for (const file of parsed.files) {
    const bytes = await transport.readFile(petId, file.relativePath);
    const digest = await sha256(bytes);
    if (digest !== file.sha256) throw new Error(`sha256 mismatch: ${file.relativePath}`);
    bytesByPath.set(file.relativePath, bytes);
  }
  const motionSpatialProfile = parsed.schemaVersion === 5
    ? parseVerifiedMotionSpatialProfile(parsed, bytesByPath)
    : undefined;
  const urls: string[] = [];
  try {
    const urlsByPath = new Map<string, string>();
    for (const file of parsed.files) {
      if (file.relativePath === parsed.modelEntry) continue;
      const bytes = bytesByPath.get(file.relativePath);
      if (bytes === undefined) throw new Error(`asset file was not loaded: ${file.relativePath}`);
      const url = URL.createObjectURL(new Blob([Uint8Array.from(bytes)], { type: mimeType(file.relativePath) }));
      urls.push(url);
      urlsByPath.set(file.relativePath, url);
    }

    const modelBytes = bytesByPath.get(parsed.modelEntry);
    if (modelBytes === undefined) throw new Error("modelEntry was not loaded");
    const rewrittenModel = rewriteModelReferences(parsed.modelEntry, modelBytes, urlsByPath);
    const modelUrl = URL.createObjectURL(new Blob([rewrittenModel], { type: "application/json" }));
    urls.push(modelUrl);
    urlsByPath.set(parsed.modelEntry, modelUrl);
    const previewUrl = urlsByPath.get(parsed.previewImage);
    if (previewUrl === undefined) throw new Error("previewImage must be distinct from modelEntry");
    const isCatManifest = parsed.schemaVersion === 4 || parsed.schemaVersion === 5;
    const blinkOverlayPath = isCatManifest
      ? parsed.files.find((file) => file.role === "blink-overlay")?.relativePath
      : undefined;
    const blinkOverlayUrl = blinkOverlayPath === undefined ? undefined : urlsByPath.get(blinkOverlayPath);
    if (blinkOverlayPath !== undefined && blinkOverlayUrl === undefined) {
      throw new Error("blink overlay file was not published");
    }
    let disposed = false;
    return {
      kind: "live2d",
      modelUrl,
      previewUrl,
      ...(isCatManifest ? { catV4: true as const } : {}),
      ...(motionSpatialProfile === undefined ? {} : { motionSpatialProfile }),
      ...(blinkOverlayUrl === undefined ? {} : { blinkOverlayUrl }),
      semantics: isCatManifest
        ? {
            motions: parsed.motions,
            expressions: {},
            hitAreas: parsed.hitAreas,
            parameters: parsed.parameters,
          } satisfies Live2DSemantics
        : parsed.semantics,
      dispose: () => { if (disposed) return; disposed = true; for (const url of urls) URL.revokeObjectURL(url); },
    };
  } catch (error) {
    for (const url of urls) URL.revokeObjectURL(url);
    throw error;
  }
}
