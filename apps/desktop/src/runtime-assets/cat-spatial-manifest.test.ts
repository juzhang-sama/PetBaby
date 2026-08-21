import { describe, expect, it } from "vitest";
import {
  buildCatSpatialManifest,
  parseCatSpatialManifest,
  type RuntimeAssetManifestV5,
} from "./cat-spatial-manifest";
import {
  CAT_EDGE_TAIL_STATES_V1,
  CAT_HIT_AREAS_V1,
  CAT_MOTION_SET_V1,
  CAT_PARAMETER_SET_V1,
} from "../runtime-live2d/cat-motion-contract";

const sha = "AB".repeat(32);

function validManifest(): RuntimeAssetManifestV5 {
  return {
    schemaVersion: 5,
    renderer: "cat-spatial-live2d-v1",
    petId: "cat-a-standard-v1",
    variantId: "standard-v1",
    skeletonVersion: "cat-a-live2d-v1",
    bodyModuleId: "body-balanced-v1",
    modelEntry: "model/cat.model3.json",
    previewImage: "preview/cat.png",
    motionSpatialProfile: "profiles/body-balanced.json",
    files: [
      { role: "model", relativePath: "model/cat.model3.json", sha256: sha },
      { role: "preview", relativePath: "preview/cat.png", sha256: sha },
      { role: "motion-spatial-profile", relativePath: "profiles/body-balanced.json", sha256: sha },
    ],
    motions: Object.fromEntries(
      CAT_MOTION_SET_V1.map((name, index) => [name, { group: "CatMotion", index }]),
    ) as RuntimeAssetManifestV5["motions"],
    parameters: Object.fromEntries(
      CAT_PARAMETER_SET_V1.map((name) => [name, `Param${name}`]),
    ) as RuntimeAssetManifestV5["parameters"],
    hitAreas: { body: "HitAreaBody", edgeTail: "HitAreaEdgeTail" },
    edgeTailStates: Object.fromEntries(
      CAT_EDGE_TAIL_STATES_V1.map((edge, index) => [
        edge,
        { group: "EdgeTail", index, tailArtMesh: "ArtMeshTail" },
      ]),
    ) as RuntimeAssetManifestV5["edgeTailStates"],
    license: {
      id: "project-owned",
      author: "PetBaby",
      source: "project",
      commercialUse: true,
      redistributable: true,
    },
  };
}

describe("cat spatial manifest v5", () => {
  it("parses a complete spatially calibrated cat package", () => {
    const parsed = parseCatSpatialManifest(validManifest());

    expect(parsed).toMatchObject({
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      bodyModuleId: "body-balanced-v1",
      motionSpatialProfile: "profiles/body-balanced.json",
    });
    expect(parsed.files[2]?.sha256).toBe("ab".repeat(32));
  });

  it("rejects an unknown body module", () => {
    expect(() => parseCatSpatialManifest({
      ...validManifest(),
      bodyModuleId: "body-unknown-v1",
    })).toThrow(/bodyModuleId/i);
  });

  it("requires the spatial profile to be listed with its manifest file role", () => {
    const manifest = validManifest();
    manifest.files = manifest.files.filter((file) => file.role !== "motion-spatial-profile");

    expect(() => parseCatSpatialManifest(manifest)).toThrow(/motionSpatialProfile.*listed.*files/i);
  });

  it("rejects traversal paths and invalid SHA-256 values", () => {
    expect(() => parseCatSpatialManifest({
      ...validManifest(),
      motionSpatialProfile: "../profiles/body-balanced.json",
    })).toThrow(/unsafe/i);

    const badHash = validManifest();
    badHash.files[2]!.sha256 = "not-a-sha256";
    expect(() => parseCatSpatialManifest(badHash)).toThrow(/sha256/i);
  });

  it("rejects builder output without a spatial profile", () => {
    const { motionSpatialProfile: _profile, ...withoutProfile } = validManifest();

    expect(() => buildCatSpatialManifest(withoutProfile)).toThrow(/motionSpatialProfile/i);
  });
});
