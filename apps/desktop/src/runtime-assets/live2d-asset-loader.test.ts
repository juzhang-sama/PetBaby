import { describe, expect, it, vi } from "vitest";
import { loadLive2DAsset } from "./live2d-asset-loader";
import type { RuntimeAssetManifestV2 } from "./live2d-manifest";

const model = new TextEncoder().encode("model");
const preview = new TextEncoder().encode("preview");
const manifest: RuntimeAssetManifestV2 = {
  schemaVersion: 2, renderer: "live2d-v1", petId: "pet-a", variantId: "v1",
  modelEntry: "model.model3.json", previewImage: "preview.png",
  files: [
    { role: "model", relativePath: "model.model3.json", sha256: "9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4" },
    { role: "preview", relativePath: "preview.png", sha256: "5975cf1bba432391c94667f5886225f69377c0aa8b9fa21fddfb21c89bcf9092" },
  ],
  semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
  license: { id: "test", author: "Test", source: "https://example.com", commercialUse: true, redistributable: false },
};

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
    expect(asset.modelUrl).toBe("blob:5");
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
});
