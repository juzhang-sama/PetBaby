import { describe, expect, it } from "vitest";
import { parseManifestV1, MANIFEST_SCHEMA_VERSION } from "./manifest-schema";

function baseManifest(): Record<string, unknown> {
  return {
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
  };
}

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

  it("accepts a manifest with rig parts", () => {
    const manifest = parseManifestV1({
      ...baseManifest(),
      parts: [
        {
          role: "main",
          relativePath: "pet.png",
          anchor: { x: 0.5, y: 1 },
          pivot: { x: 0.5, y: 0.5 },
          zIndex: 0,
          deformable: true,
          boneId: "spine",
        },
      ],
    });
    expect(manifest.parts).toHaveLength(1);
    expect(manifest.parts?.[0]).toMatchObject({
      role: "main",
      zIndex: 0,
      deformable: true,
      boneId: "spine",
    });
  });

  it("rejects a part whose anchor or pivot is outside 0..1", () => {
    expect(() => parseManifestV1({
      ...baseManifest(),
      parts: [
        {
          role: "main",
          relativePath: "pet.png",
          anchor: { x: 1.5, y: 0 },
          pivot: { x: 0.5, y: 0.5 },
          zIndex: 0,
          deformable: true,
        },
      ],
    })).toThrow(/anchor|pivot/i);
  });

  it("rejects duplicate part roles", () => {
    expect(() => parseManifestV1({
      ...baseManifest(),
      parts: [
        {
          role: "main",
          relativePath: "pet.png",
          anchor: { x: 0.5, y: 1 },
          pivot: { x: 0.5, y: 0.5 },
          zIndex: 0,
          deformable: true,
        },
        {
          role: "main",
          relativePath: "other.png",
          anchor: { x: 0.5, y: 1 },
          pivot: { x: 0.5, y: 0.5 },
          zIndex: 1,
          deformable: false,
        },
      ],
    })).toThrow(/duplicate|role/i);
  });

  it("rejects a non-integer zIndex", () => {
    expect(() => parseManifestV1({
      ...baseManifest(),
      parts: [
        {
          role: "main",
          relativePath: "pet.png",
          anchor: { x: 0.5, y: 1 },
          pivot: { x: 0.5, y: 0.5 },
          zIndex: 0.5,
          deformable: true,
        },
      ],
    })).toThrow(/zIndex/i);
  });

  it("rejects an empty parts array", () => {
    expect(() => parseManifestV1({
      ...baseManifest(),
      parts: [],
    })).toThrow(/at least one part/i);
  });

  it("rejects an empty boneId", () => {
    expect(() => parseManifestV1({
      ...baseManifest(),
      parts: [
        {
          role: "main",
          relativePath: "pet.png",
          anchor: { x: 0.5, y: 1 },
          pivot: { x: 0.5, y: 0.5 },
          zIndex: 0,
          deformable: true,
          boneId: "",
        },
      ],
    })).toThrow(/boneId/i);
  });

  it("accepts a null boneId and normalizes it to undefined", () => {
    const manifest = parseManifestV1({
      ...baseManifest(),
      parts: [
        {
          role: "main",
          relativePath: "pet.png",
          anchor: { x: 0.5, y: 1 },
          pivot: { x: 0.5, y: 0.5 },
          zIndex: 0,
          deformable: true,
          boneId: null,
        },
      ],
    });
    expect(manifest.parts?.[0]?.boneId).toBeUndefined();
  });

  it("accepts mesh features and returns them", () => {
    const box = { x: 0.2, y: 0.3, width: 0.1, height: 0.08 };
    const manifest = parseManifestV1({
      ...baseManifest(),
      meshFeatures: {
        leftEye: box,
        rightEye: { ...box, x: 0.7 },
        leftEar: box,
        rightEar: box,
        tail: box,
      },
    });
    expect(manifest.meshFeatures?.leftEye).toEqual(box);
    expect(manifest.meshFeatures?.rightEye.x).toBe(0.7);
  });

  it("rejects mesh features with out-of-range boxes", () => {
    const box = { x: 0.2, y: 0.3, width: 0.1, height: 0.08 };
    expect(() => parseManifestV1({
      ...baseManifest(),
      meshFeatures: {
        leftEye: { ...box, width: 1.2 },
        rightEye: box,
        leftEar: box,
        rightEar: box,
        tail: box,
      },
    })).toThrow(/meshFeatures/i);
  });

  it("accepts a manifest without mesh features", () => {
    const manifest = parseManifestV1(baseManifest());
    expect(manifest.meshFeatures).toBeUndefined();
  });
});
