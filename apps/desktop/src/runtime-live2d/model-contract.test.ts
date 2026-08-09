import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { RuntimeAssetManifestV2 } from "../runtime-assets/live2d-manifest";
import { parseLive2DManifest } from "../runtime-assets/live2d-manifest";
import { validateModelContract } from "./model-contract";

function modelFixture(overrides: Partial<RuntimeAssetManifestV2> = {}): RuntimeAssetManifestV2 {
  return {
    schemaVersion: 2,
    renderer: "live2d-v1",
    petId: "builtin-reference",
    variantId: "front-sitting-v1",
    modelEntry: "pet.model3.json",
    previewImage: "preview.png",
    files: [
      { role: "model", relativePath: "pet.model3.json", sha256: "ab".repeat(32) },
      { role: "moc", relativePath: "pet.moc3", sha256: "cd".repeat(32) },
      { role: "texture", relativePath: "texture_00.png", sha256: "ef".repeat(32) },
      { role: "preview", relativePath: "preview.png", sha256: "12".repeat(32) },
      { role: "motion", relativePath: "motions/idle.motion3.json", sha256: "34".repeat(32) },
      { role: "expression", relativePath: "expressions/neutral.exp3.json", sha256: "56".repeat(32) },
    ],
    semantics: {
      motions: {
        idle: { group: "Idle" },
        "react-happy": { group: "ReactHappy" },
        "react-curious": { group: "ReactCurious" },
        sleep: { group: "Sleep" },
        wake: { group: "Wake" },
        carried: { group: "Carried" },
        landed: { group: "Landed" },
      },
      expressions: {
        neutral: "Neutral",
        happy: "Happy",
        curious: "Curious",
        sleepy: "Sleepy",
        sad: "Sad",
        angry: "Angry",
      },
      hitAreas: { head: "Head", body: "Body" },
      parameters: {
        eyeOpen: "ParamEyeOpen",
        angleX: "ParamAngleX",
        angleY: "ParamAngleY",
        mouthOpen: "ParamMouthOpenY",
        bodyBreath: "ParamBreath",
      },
    },
    license: {
      id: "builtin-reference",
      author: "PetBaby",
      source: "internal",
      commercialUse: true,
      redistributable: true,
    },
    ...overrides,
  };
}

describe("validateModelContract", () => {
  it("accepts the packaged pet with only breathing and body sway mappings", () => {
    const manifestUrl = new URL("../../public/builtin-pets/pet-live2d-v1/manifest.json", import.meta.url);
    const manifest = parseLive2DManifest(JSON.parse(readFileSync(manifestUrl, "utf8")));

    expect(validateModelContract(manifest)).toEqual({ valid: true, errors: [] });
    expect(manifest.semantics.parameters).toEqual({
      bodyBreath: "ParamBreath",
      bodySway: "ParamBodyAngleX",
    });
    expect(manifest.semantics.motions).toEqual({});
    expect(manifest.semantics.expressions).toEqual({});
    expect(manifest.semantics.hitAreas).toEqual({});
  });

  it("accepts a loadable micro-motion model without motions or expressions", () => {
    const base = modelFixture();
    const fixture = modelFixture({
      files: base.files.filter((file) => !file.relativePath.endsWith(".motion3.json") && !file.relativePath.endsWith(".exp3.json")),
      semantics: {
        motions: {},
        expressions: {},
        hitAreas: {},
        parameters: {
          bodyBreath: "ParamBreath",
          bodySway: "ParamBodyAngleX",
        },
      },
    });

    expect(validateModelContract(fixture)).toEqual({ valid: true, errors: [] });
  });

  it("treats motions, expressions, hit areas, and parameters as optional capabilities", () => {
    const fixture = modelFixture({
      semantics: {
        motions: {},
        expressions: {},
        hitAreas: {},
        parameters: {},
      },
    });

    expect(validateModelContract(fixture)).toEqual({ valid: true, errors: [] });
  });

  it("requires the files needed to load and preview a model", () => {
    const fixture = modelFixture({
      files: [{ role: "preview", relativePath: "preview.png", sha256: "ab".repeat(32) }],
      semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
    });

    const result = validateModelContract(fixture);

    expect(result.errors).toContain("missing file: model3");
    expect(result.errors).toContain("missing file: moc3");
    expect(result.errors).toContain("missing file: texture");
    expect(result.errors).not.toContain("missing motion: idle");
    expect(result.errors).not.toContain("missing hit area: head");
    expect(result.errors).not.toContain("missing parameter: mouthOpen");
    expect(result.valid).toBe(false);
  });

  it("accepts a complete front-sitting model contract", () => {
    const result = validateModelContract(modelFixture());

    expect(result).toEqual({ valid: true, errors: [] });
  });
});
