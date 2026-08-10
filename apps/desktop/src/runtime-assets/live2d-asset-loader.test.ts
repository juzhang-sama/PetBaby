import { afterEach, describe, expect, it, vi } from "vitest";
import { loadLive2DAsset } from "./live2d-asset-loader";
import type { RuntimeAssetManifestV2 } from "./live2d-manifest";

const model = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
const preview = new TextEncoder().encode("preview");
const manifest: RuntimeAssetManifestV2 = {
  schemaVersion: 2, renderer: "live2d-v1", petId: "pet-a", variantId: "v1",
  modelEntry: "model.model3.json", previewImage: "preview.png",
  files: [
    { role: "model", relativePath: "model.model3.json", sha256: "3d8da9c29b013f27dac037b04727f79eeb72029654714f704de74e9679f681d6" },
    { role: "preview", relativePath: "preview.png", sha256: "5975cf1bba432391c94667f5886225f69377c0aa8b9fa21fddfb21c89bcf9092" },
  ],
  semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
  license: { id: "test", author: "Test", source: "https://example.com", commercialUse: true, redistributable: false },
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe("loadLive2DAsset", () => {
  it("validates every digest before creating object URLs", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:model");
    const transport = { readManifest: vi.fn(async () => manifest), readFile: vi.fn(async (_petId: string, path: string) => path === "model.model3.json" ? model : new TextEncoder().encode("corrupt")) };
    await expect(loadLive2DAsset("pet-a", manifest, transport)).rejects.toThrow(/sha256/i);
    expect(create).not.toHaveBeenCalled();
    create.mockRestore();
  });

  it("disposes every URL after a successful load", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => `blob:${(blob as Blob).size}`);
    const transport = { readManifest: vi.fn(async () => manifest), readFile: vi.fn(async (_petId: string, path: string) => path === "model.model3.json" ? model : preview) };
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const asset = await loadLive2DAsset("pet-a", manifest, transport);
    expect(asset.modelUrl).toMatch(/^blob:/);
    expect(asset.previewUrl).toBe("blob:7");
    asset.dispose();
    asset.dispose();
    expect(revoke).toHaveBeenCalledTimes(2);
    create.mockRestore();
    revoke.mockRestore();
  });

  it("revokes URLs already created when later URL creation fails", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValueOnce("blob:model").mockImplementationOnce(() => { throw new Error("URL failed"); });
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const transport = { readManifest: vi.fn(async () => manifest), readFile: vi.fn(async (_petId: string, path: string) => path === "model.model3.json" ? model : preview) };
    await expect(loadLive2DAsset("pet-a", manifest, transport)).rejects.toThrow("URL failed");
    expect(revoke).toHaveBeenCalledWith("blob:model");
    create.mockRestore();
    revoke.mockRestore();
  });

  it("publishes every declared resource and rewrites model references to object URLs", async () => {
    const encoder = new TextEncoder();
    const resources = new Map<string, Uint8Array>([
      ["model.model3.json", encoder.encode(JSON.stringify({
        Version: 3,
        FileReferences: {
          Moc: "pet.moc3",
          Textures: ["textures/body.png"],
          Motions: { Idle: [{ File: "motions/idle.motion3.json" }] },
        },
      }))],
      ["pet.moc3", encoder.encode("moc")],
      ["textures/body.png", encoder.encode("texture")],
      ["motions/idle.motion3.json", encoder.encode("motion")],
      ["preview.png", encoder.encode("preview")],
    ]);
    const files = await Promise.all([...resources].map(async ([relativePath, bytes]) => ({
      role: relativePath === "preview.png" ? "preview" : "model-resource",
      relativePath,
      sha256: await digest(bytes),
    })));
    const packageManifest: RuntimeAssetManifestV2 = { ...manifest, files };
    const blobs = new Map<string, Blob>();
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const create = vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => {
      const url = `blob:${blobs.size}`;
      blobs.set(url, blob as Blob);
      return url;
    });
    const transport = {
      readManifest: vi.fn(async () => packageManifest),
      readFile: vi.fn(async (_petId: string, path: string) => resources.get(path)!),
    };

    const asset = await loadLive2DAsset("pet-a", packageManifest, transport);
    const publishedModel = JSON.parse(await blobs.get(asset.modelUrl)!.text());

    expect(create).toHaveBeenCalledTimes(resources.size);
    expect(publishedModel.FileReferences.Moc).toMatch(/^blob:/);
    expect(publishedModel.FileReferences.Textures[0]).toMatch(/^blob:/);
    expect(publishedModel.FileReferences.Motions.Idle[0].File).toMatch(/^blob:/);
    asset.dispose();
    asset.dispose();
    expect(revoke).toHaveBeenCalledTimes(resources.size);
    create.mockRestore();
  });

  it("rejects a Tauri manifest that differs from the caller manifest", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:unused");
    const changed = {
      ...manifest,
      semantics: { ...manifest.semantics, expressions: { happy: "Happy" } },
    };
    const transport = {
      readManifest: vi.fn(async () => changed),
      readFile: vi.fn(async () => model),
    };

    await expect(loadLive2DAsset("pet-a", manifest, transport)).rejects.toThrow(/manifest mismatch/i);
    expect(transport.readFile).not.toHaveBeenCalled();
    expect(create).not.toHaveBeenCalled();
    create.mockRestore();
  });

  it("creates blobs from the verified Uint8Array view only", async () => {
    const modelJson = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
    const backing = new TextEncoder().encode("prefix-preview-suffix");
    const previewView = backing.subarray(7, 14);
    const exactManifest: RuntimeAssetManifestV2 = {
      ...manifest,
      files: [
        { role: "model", relativePath: manifest.modelEntry, sha256: await digest(modelJson) },
        { role: "preview", relativePath: manifest.previewImage, sha256: await digest(previewView) },
      ],
    };
    const blobs = new Map<string, Blob>();
    const create = vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => {
      const url = `blob:${blobs.size}`;
      blobs.set(url, blob as Blob);
      return url;
    });
    const transport = {
      readManifest: vi.fn(async () => exactManifest),
      readFile: vi.fn(async (_petId: string, path: string) => path === manifest.modelEntry ? modelJson : previewView),
    };

    const asset = await loadLive2DAsset("pet-a", exactManifest, transport);

    expect(blobs.get(asset.previewUrl)?.size).toBe(previewView.byteLength);
    expect(await blobs.get(asset.previewUrl)?.text()).toBe("preview");
    asset.dispose();
    create.mockRestore();
  });
});

async function digest(bytes: Uint8Array): Promise<string> {
  const copy = Uint8Array.from(bytes);
  const value = await crypto.subtle.digest("SHA-256", copy);
  return [...new Uint8Array(value)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
