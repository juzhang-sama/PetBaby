import { describe, expect, it } from "vitest";
import { parseLive2DManifest, normalizeAssetPath } from "./live2d-manifest";

const sha = "ab".repeat(32);
const valid = {
  schemaVersion: 2,
  renderer: "live2d-v1",
  petId: "pet-a",
  variantId: "v1",
  modelEntry: "models/pet.model3.json",
  previewImage: "preview/pet.png",
  files: [
    { role: "model", relativePath: "models/pet.model3.json", sha256: sha },
    { role: "preview", relativePath: "preview/pet.png", sha256: sha },
  ],
  semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
  license: { id: "test", author: "Pet Studio", source: "https://example.com", commercialUse: true, redistributable: false },
};

describe("Live2D manifest v2", () => {
  it("parses a valid v2 manifest", () => {
    expect(parseLive2DManifest(valid).modelEntry).toBe("models/pet.model3.json");
  });
  it("rejects traversal, absolute and drive paths", () => {
    for (const path of ["../model.json", "/model.json", "C:/model.json", "models//model.json", "models/./model.json"]) {
      expect(() => normalizeAssetPath(path)).toThrow();
    }
  });
  it("normalizes backslash separators", () => {
    expect(normalizeAssetPath("models\\pet.model3.json")).toBe("models/pet.model3.json");
  });
  it("rejects unknown extensions", () => {
    expect(() => parseLive2DManifest({ ...valid, modelEntry: "models/pet.exe" })).toThrow(/extension/i);
  });
  it("reports missing required fields and license", () => {
    expect(() => parseLive2DManifest({ ...valid, previewImage: undefined })).toThrow(/previewImage/i);
    expect(() => parseLive2DManifest({ ...valid, license: undefined })).toThrow(/license/i);
  });
  it("rejects unknown semantic keys", () => {
    expect(() => parseLive2DManifest({ ...valid, semantics: { ...valid.semantics, motions: { dance: { group: "Dance" } } } })).toThrow(/unknown semantics/i);
  });
  it("rejects an invalid sha summary", () => {
    expect(() => parseLive2DManifest({ ...valid, files: [{ role: "model", relativePath: valid.modelEntry, sha256: "nope" }] })).toThrow(/sha256/i);
  });
});
