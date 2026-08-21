import { describe, expect, it } from "vitest";
import {
  CAT_CHARACTER_RENDERER,
  CAT_SKELETON_VERSION,
  parseCatCharacterManifest,
  type RuntimeAssetManifestV4,
} from "./cat-character-manifest";
import {
  CAT_EDGE_TAIL_STATES_V1,
  CAT_HIT_AREAS_V1,
  CAT_MOTION_SET_V1,
  CAT_PARAMETER_SET_V1,
} from "../runtime-live2d/cat-motion-contract";

const sha = "AB".repeat(32);

function validManifest(): RuntimeAssetManifestV4 {
  return {
    schemaVersion: 4,
    renderer: CAT_CHARACTER_RENDERER,
    petId: "cat-a-standard-v1",
    variantId: "standard-v1",
    skeletonVersion: CAT_SKELETON_VERSION,
    modelEntry: "model/cat.model3.json",
    previewImage: "preview/cat.png",
    files: [
      { role: "model", relativePath: "model/cat.model3.json", sha256: sha },
      { role: "preview", relativePath: "preview/cat.png", sha256: sha },
    ],
    motions: Object.fromEntries(
      CAT_MOTION_SET_V1.map((name, index) => [name, { group: "CatMotion", index }]),
    ) as RuntimeAssetManifestV4["motions"],
    parameters: Object.fromEntries(
      CAT_PARAMETER_SET_V1.map((name) => [name, `Param${name}`]),
    ) as RuntimeAssetManifestV4["parameters"],
    hitAreas: { body: "HitAreaBody", edgeTail: "HitAreaEdgeTail" },
    edgeTailStates: Object.fromEntries(
      CAT_EDGE_TAIL_STATES_V1.map((edge, index) => [
        edge,
        { group: "EdgeTail", index, tailArtMesh: "ArtMeshTail" },
      ]),
    ) as RuntimeAssetManifestV4["edgeTailStates"],
    license: {
      id: "project-owned",
      author: "PetBaby",
      source: "project",
      commercialUse: true,
      redistributable: true,
    },
  };
}

describe("cat character manifest v4", () => {
  it("parses and normalizes the complete cat-a-live2d-v1 contract", () => {
    const parsed = parseCatCharacterManifest(validManifest());
    expect(parsed.skeletonVersion).toBe("cat-a-live2d-v1");
    expect(parsed.files[0]?.sha256).toBe("ab".repeat(32));
    expect(Object.keys(parsed.motions)).toEqual(CAT_MOTION_SET_V1);
    expect(Object.keys(parsed.parameters)).toEqual(CAT_PARAMETER_SET_V1);
    expect(Object.keys(parsed.hitAreas)).toEqual(CAT_HIT_AREAS_V1);
    expect(Object.keys(parsed.edgeTailStates)).toEqual(CAT_EDGE_TAIL_STATES_V1);
  });

  it("rejects the wrong skeleton and renderer", () => {
    expect(() => parseCatCharacterManifest({ ...validManifest(), skeletonVersion: "cat-v0" })).toThrow(/skeletonVersion/i);
    expect(() => parseCatCharacterManifest({ ...validManifest(), renderer: "live2d-v1" })).toThrow(/renderer/i);
  });

  it.each(CAT_MOTION_SET_V1)("rejects a package missing motion %s", (name) => {
    const manifest = validManifest();
    delete (manifest.motions as Partial<typeof manifest.motions>)[name];
    expect(() => parseCatCharacterManifest(manifest)).toThrow(new RegExp(name, "i"));
  });

  it.each(CAT_PARAMETER_SET_V1)("rejects a package missing parameter %s", (name) => {
    const manifest = validManifest();
    delete (manifest.parameters as Partial<typeof manifest.parameters>)[name];
    expect(() => parseCatCharacterManifest(manifest)).toThrow(new RegExp(name, "i"));
  });

  it.each(CAT_EDGE_TAIL_STATES_V1)("rejects a package missing edge-tail state %s", (edge) => {
    const manifest = validManifest();
    delete (manifest.edgeTailStates as Partial<typeof manifest.edgeTailStates>)[edge];
    expect(() => parseCatCharacterManifest(manifest)).toThrow(new RegExp(edge, "i"));
  });

  it("requires all edge states to reuse one complete tail ArtMesh", () => {
    const manifest = validManifest();
    manifest.edgeTailStates.right.tailArtMesh = "ArtMeshTailRightScreenshot";
    expect(() => parseCatCharacterManifest(manifest)).toThrow(/same tail ArtMesh/i);
  });

  it("rejects unknown motion, parameter, hit-area, and edge semantics", () => {
    for (const group of ["motions", "parameters", "hitAreas", "edgeTailStates"] as const) {
      const manifest = validManifest() as unknown as Record<string, Record<string, unknown>>;
      manifest[group]!.unknown = group === "parameters" ? "ParamUnknown" : {};
      expect(() => parseCatCharacterManifest(manifest)).toThrow(/unknown/i);
    }
  });

  it("rejects unsafe, duplicate, unlisted, and invalid-hash file paths", () => {
    const traversal = validManifest();
    traversal.modelEntry = "../cat.model3.json";
    expect(() => parseCatCharacterManifest(traversal)).toThrow(/unsafe/i);

    const duplicate = validManifest();
    duplicate.files[1]!.relativePath = "model\\cat.model3.json";
    expect(() => parseCatCharacterManifest(duplicate)).toThrow(/duplicate/i);

    const unlisted = validManifest();
    unlisted.previewImage = "preview/missing.png";
    expect(() => parseCatCharacterManifest(unlisted)).toThrow(/listed in files/i);

    const badHash = validManifest();
    badHash.files[0]!.sha256 = "nope";
    expect(() => parseCatCharacterManifest(badHash)).toThrow(/sha256/i);
  });

  it("rejects a license that cannot be redistributed", () => {
    const manifest = validManifest();
    manifest.license.redistributable = false;
    expect(() => parseCatCharacterManifest(manifest)).toThrow(/redistributable/i);
  });
});
