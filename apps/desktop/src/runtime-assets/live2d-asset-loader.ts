import { invoke } from "@tauri-apps/api/core";
import type { Live2DSemantics, PetRenderAsset } from "../runtime/pet-renderer";
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
  const copy = new Uint8Array(bytes);
  const digest = await crypto.subtle.digest("SHA-256", copy as unknown as BufferSource);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

export async function loadLive2DAsset(
  petId: string,
  expectedManifest: RuntimeAssetManifestV2,
  transport: Live2DAssetTransport = defaultTransport,
): Promise<Extract<PetRenderAsset, { kind: "live2d" }>> {
  const expected = parseLive2DManifest(expectedManifest);
  if (expected.petId !== petId) throw new Error("manifest identity mismatch");
  const parsed = parseLive2DManifest(await transport.readManifest(petId));
  if (parsed.petId !== petId || parsed.variantId !== expected.variantId || parsed.modelEntry !== expected.modelEntry || parsed.previewImage !== expected.previewImage) throw new Error("manifest identity mismatch");
  const bytesByPath = new Map<string, Uint8Array>();
  for (const file of parsed.files) {
    const bytes = await transport.readFile(petId, file.relativePath);
    const digest = await sha256(bytes);
    if (digest !== file.sha256) throw new Error(`sha256 mismatch: ${file.relativePath}`);
    bytesByPath.set(file.relativePath, bytes);
  }
  const modelBytes = bytesByPath.get(parsed.modelEntry);
  const previewBytes = bytesByPath.get(parsed.previewImage);
  if (!modelBytes || !previewBytes) throw new Error("manifest entries were not loaded");
  const urls: string[] = [];
  try {
    const modelUrl = URL.createObjectURL(new Blob([modelBytes.buffer as ArrayBuffer]));
    urls.push(modelUrl);
    const previewUrl = URL.createObjectURL(new Blob([previewBytes.buffer as ArrayBuffer]));
    urls.push(previewUrl);
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
