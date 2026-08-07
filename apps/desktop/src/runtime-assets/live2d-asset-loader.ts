import { invoke } from "@tauri-apps/api/core";
import type { PetRenderAsset } from "../runtime/pet-renderer";
import { parseLive2DManifest, type RuntimeAssetManifestV2 } from "./live2d-manifest";

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

export async function loadLive2DAsset(
  petId: string,
  expectedManifest: RuntimeAssetManifestV2,
  transport: Live2DAssetTransport = defaultTransport,
): Promise<Extract<PetRenderAsset, { kind: "live2d" }>> {
  const expected = parseLive2DManifest(expectedManifest);
  if (expected.petId !== petId) throw new Error("manifest mismatch");
  const parsed = parseLive2DManifest(await transport.readManifest(petId));
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
    let disposed = false;
    return {
      kind: "live2d",
      modelUrl,
      previewUrl,
      semantics: parsed.semantics,
      dispose: () => { if (disposed) return; disposed = true; for (const url of urls) URL.revokeObjectURL(url); },
    };
  } catch (error) {
    for (const url of urls) URL.revokeObjectURL(url);
    throw error;
  }
}
