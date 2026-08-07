import { describe, expect, it } from "vitest";
import type { RuntimeAssetManifestV2 } from "../runtime-assets/live2d-manifest";
import { createDirectAdoptionEntry } from "./adoption";

const manifest = {
  schemaVersion: 2,
  renderer: "live2d-v1",
  petId: "builtin-reference",
  variantId: "front-sitting-v1",
  modelEntry: "pet.model3.json",
  previewImage: "preview.png",
  files: [
    { role: "model", relativePath: "pet.model3.json", sha256: "ab".repeat(32) },
    { role: "moc", relativePath: "pet.moc3", sha256: "cd".repeat(32) },
    { role: "texture", relativePath: "texture.png", sha256: "ef".repeat(32) },
    { role: "preview", relativePath: "preview.png", sha256: "12".repeat(32) },
    { role: "motion", relativePath: "idle.motion3.json", sha256: "34".repeat(32) },
    { role: "expression", relativePath: "neutral.exp3.json", sha256: "56".repeat(32) },
  ],
  semantics: {
    motions: {
      idle: { group: "Idle" }, "react-happy": { group: "Happy" }, "react-curious": { group: "Curious" },
      sleep: { group: "Sleep" }, wake: { group: "Wake" }, carried: { group: "Carried" }, landed: { group: "Landed" },
    },
    expressions: { neutral: "Neutral", happy: "Happy", curious: "Curious", sleepy: "Sleepy", sad: "Sad", angry: "Angry" },
    hitAreas: { head: "Head", body: "Body" },
    parameters: { eyeOpen: "EyeOpen", angleX: "AngleX", angleY: "AngleY", mouthOpen: "MouthOpen", bodyBreath: "Breath" },
  },
  license: { id: "model-license", author: "Author", source: "https://example.com", commercialUse: true, redistributable: true },
} satisfies RuntimeAssetManifestV2;

describe("createDirectAdoptionEntry", () => {
  it("requires a reviewed commercial redistribution record", () => {
    expect(() => createDirectAdoptionEntry({
      manifest,
      modelId: "builtin-reference",
      displayName: "Reference Pet",
      manifestPath: "builtin-pets/live2d-reference/manifest.json",
      licensePath: "builtin-pets/live2d-reference/许可证.json",
      licenseReview: {
        modelId: "builtin-reference",
        author: "Author",
        source: "https://example.com",
        commercialUse: false,
        redistribution: false,
        reviewedAt: "",
      },
    })).toThrow(/license/i);
  });

  it("returns a validated Live2D direct adoption entry", () => {
    const entry = createDirectAdoptionEntry({
      manifest,
      modelId: "builtin-reference",
      displayName: "Reference Pet",
      manifestPath: "builtin-pets/live2d-reference/manifest.json",
      licensePath: "builtin-pets/live2d-reference/许可证.json",
      licenseReview: {
        modelId: "builtin-reference",
        author: "Author",
        source: "https://example.com",
        commercialUse: true,
        redistribution: true,
        reviewedAt: "2026-08-07",
      },
    });

    expect(entry.renderer).toBe("live2d-v1");
    expect(entry.manifest.petId).toBe("builtin-reference");
    expect(entry.licenseReview.redistribution).toBe(true);
  });

  it("rejects a license review that does not match the manifest", () => {
    expect(() => createDirectAdoptionEntry({
      manifest,
      modelId: "builtin-reference",
      displayName: "Reference Pet",
      manifestPath: "builtin-pets/live2d-reference/manifest.json",
      licensePath: "builtin-pets/live2d-reference/许可证.json",
      licenseReview: {
        modelId: "builtin-reference",
        author: "Different Author",
        source: "https://example.com",
        commercialUse: true,
        redistribution: true,
        reviewedAt: "2026-08-07",
      },
    })).toThrow(/does not match/i);
  });

  it("rejects a manifest that fails the runtime v2 schema", () => {
    expect(() => createDirectAdoptionEntry({
      manifest: {
        ...manifest,
        files: manifest.files.map((file, index) => index === 0 ? { ...file, sha256: "bad" } : file),
      },
      modelId: "builtin-reference",
      displayName: "Reference Pet",
      manifestPath: "builtin-pets/live2d-reference/manifest.json",
      licensePath: "builtin-pets/live2d-reference/许可证.json",
      licenseReview: {
        modelId: "builtin-reference",
        author: "Author",
        source: "https://example.com",
        commercialUse: true,
        redistribution: true,
        reviewedAt: "2026-08-07",
      },
    })).toThrow(/sha256/i);
  });

  it("rejects an invalid license review date", () => {
    expect(() => createDirectAdoptionEntry({
      manifest,
      modelId: "builtin-reference",
      displayName: "Reference Pet",
      manifestPath: "builtin-pets/live2d-reference/manifest.json",
      licensePath: "builtin-pets/live2d-reference/许可证.json",
      licenseReview: {
        modelId: "builtin-reference",
        author: "Author",
        source: "https://example.com",
        commercialUse: true,
        redistribution: true,
        reviewedAt: "not-a-date",
      },
    })).toThrow(/review date/i);
  });
});
