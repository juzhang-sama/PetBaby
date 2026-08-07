import { describe, expect, it } from "vitest";
import { parseManifestV1, parseRuntimeAssetManifest, MANIFEST_SCHEMA_VERSION } from "./manifest-schema";

describe("parseManifestV1", () => {
  it("accepts a valid manifest", () => {
    const manifest = parseManifestV1({
      schemaVersion: 1,
      assetType: "single-image",
      petId: "pet-1",
      variantId: "variant-1",
      styleId: "signature-cartoon-v1",
      view: "front",
      pose: "sitting",
      files: [
        { role: "main", relativePath: "pet.png", sha256: "ab".repeat(32) },
      ],
      animation: { idleFps: 12, blinkMsMin: 3000, blinkMsMax: 8000 },
    });
    expect(manifest.petId).toBe("pet-1");
    expect(manifest.files[0]?.sha256).toHaveLength(64);
  });

  it("rejects an unknown schema version", () => {
    expect(() => parseManifestV1({
      schemaVersion: 2,
      assetType: "single-image",
      petId: "pet-1",
      variantId: "variant-1",
      styleId: "signature-cartoon-v1",
      view: "front",
      pose: "sitting",
      files: [],
      animation: { idleFps: 12, blinkMsMin: 3000, blinkMsMax: 8000 },
    })).toThrow(/schemaVersion/i);
  });

  it("rejects a manifest with an invalid sha256", () => {
    expect(() => parseManifestV1({
      schemaVersion: 1,
      assetType: "single-image",
      petId: "pet-1",
      variantId: "variant-1",
      styleId: "signature-cartoon-v1",
      view: "front",
      pose: "sitting",
      files: [{ role: "main", relativePath: "pet.png", sha256: "zz" }],
      animation: { idleFps: 12, blinkMsMin: 3000, blinkMsMax: 8000 },
    })).toThrow(/sha256/i);
  });

  it("pins the schema version constant", () => {
    expect(MANIFEST_SCHEMA_VERSION).toBe(1);
  });

  it("keeps v1 limited to static PNG fallback", () => {
    expect(() => parseManifestV1({
      schemaVersion: 1, assetType: "single-image", petId: "p", variantId: "v", styleId: "s", view: "front", pose: "sitting",
      files: [{ role: "main", relativePath: "pet.model3.json", sha256: "ab".repeat(32) }], animation: { idleFps: 1, blinkMsMin: 1, blinkMsMax: 2 },
    })).toThrow(/PNG/i);
  });

  it("rejects unsafe v1 asset paths", () => {
    expect(() => parseManifestV1({
      schemaVersion: 1, assetType: "single-image", petId: "p", variantId: "v", styleId: "s", view: "front", pose: "sitting",
      files: [{ role: "main", relativePath: "../pet.png", sha256: "ab".repeat(32) }], animation: { idleFps: 1, blinkMsMin: 1, blinkMsMax: 2 },
    })).toThrow(/asset path/i);
  });

  it("dispatches schema v2 manifests", () => {
    expect(parseRuntimeAssetManifest({
      schemaVersion: 2, renderer: "live2d-v1", petId: "p", variantId: "v",
      modelEntry: "model.model3.json", previewImage: "preview.png",
      files: [
        { role: "model", relativePath: "model.model3.json", sha256: "ab".repeat(32) },
        { role: "preview", relativePath: "preview.png", sha256: "ab".repeat(32) },
      ], semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
      license: { id: "test", author: "test", source: "test", commercialUse: true, redistributable: false },
    }).schemaVersion).toBe(2);
  });
});
