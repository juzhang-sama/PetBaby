import { describe, expect, it } from "vitest";
import { parseManifestV1, parseRuntimeAssetManifest, MANIFEST_SCHEMA_VERSION } from "./manifest-schema";
import { validAnimatedManifest } from "./animated-image-test-fixtures";
import type { RuntimeAssetManifestV5 } from "../runtime-assets/cat-spatial-manifest";

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

  it("dispatches schema v3 animated image manifests", () => {
    expect(parseRuntimeAssetManifest(validAnimatedManifest())).toMatchObject({
      schemaVersion: 3,
      renderer: "animated-image-v1",
    });
  });

  it("dispatches a complete schema v4 cat character manifest", () => {
    const motions = Object.fromEntries([
      "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
      "sleepy-yawn", "half-stand-stretch",
    ].map((name) => [name, { group: name, index: 0 }]));
    const parameters = Object.fromEntries([
      "eyeOpenLeft", "eyeOpenRight", "eyeBallX", "eyeBallY", "earLeft", "earRight",
      "tailAngle", "tailCurl", "tailTip", "bodyBreath", "bodyStretch", "mouthOpen",
    ].map((name) => [name, `Param-${name}`]));
    const edgeTailStates = Object.fromEntries(
      ["left", "right", "top", "bottom"].map((name) => [name, {
        group: `edge-tail-${name}`, index: 0, tailArtMesh: "ArtMeshTail",
      }]),
    );

    expect(parseRuntimeAssetManifest({
      schemaVersion: 4,
      renderer: "cat-live2d-v1",
      petId: "cat-a-standard-v1",
      variantId: "standard-v1",
      skeletonVersion: "cat-a-live2d-v1",
      modelEntry: "cat.model3.json",
      previewImage: "preview.png",
      files: [
        { role: "model", relativePath: "cat.model3.json", sha256: "ab".repeat(32) },
        { role: "preview", relativePath: "preview.png", sha256: "cd".repeat(32) },
      ],
      motions,
      parameters,
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates,
      license: { id: "project", author: "PetBaby", source: "project", commercialUse: true, redistributable: true },
    })).toMatchObject({ schemaVersion: 4, renderer: "cat-live2d-v1" });
  });

  it("dispatches a complete schema v5 spatial cat character manifest", () => {
    const motions = Object.fromEntries([
      "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
      "sleepy-yawn", "half-stand-stretch",
    ].map((name) => [name, { group: name, index: 0 }]));
    const parameters = Object.fromEntries([
      "eyeOpenLeft", "eyeOpenRight", "eyeBallX", "eyeBallY", "earLeft", "earRight",
      "tailAngle", "tailCurl", "tailTip", "bodyBreath", "bodyStretch", "mouthOpen",
    ].map((name) => [name, `Param-${name}`]));
    const edgeTailStates = Object.fromEntries(
      ["left", "right", "top", "bottom"].map((name) => [name, {
        group: `edge-tail-${name}`, index: 0, tailArtMesh: "ArtMeshTail",
      }]),
    );
    const manifest: RuntimeAssetManifestV5 = {
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "cat-a-standard-v1",
      variantId: "standard-v1",
      skeletonVersion: "cat-a-live2d-v1",
      bodyModuleId: "body-balanced-v1",
      modelEntry: "cat.model3.json",
      previewImage: "preview.png",
      motionSpatialProfile: "profiles/body-balanced.json",
      files: [
        { role: "model", relativePath: "cat.model3.json", sha256: "ab".repeat(32) },
        { role: "preview", relativePath: "preview.png", sha256: "cd".repeat(32) },
        { role: "motion-spatial-profile", relativePath: "profiles/body-balanced.json", sha256: "ef".repeat(32) },
      ],
      motions: motions as RuntimeAssetManifestV5["motions"],
      parameters: parameters as RuntimeAssetManifestV5["parameters"],
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates: edgeTailStates as RuntimeAssetManifestV5["edgeTailStates"],
      license: { id: "project", author: "PetBaby", source: "project", commercialUse: true, redistributable: true },
    };

    expect(parseRuntimeAssetManifest(manifest)).toMatchObject({
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      bodyModuleId: "body-balanced-v1",
    });
  });

  it("rejects unsupported schema versions instead of treating them as v1", () => {
    expect(() => parseRuntimeAssetManifest({ schemaVersion: 6 })).toThrow(/schemaVersion/i);
  });
});
