import type { MotionProfileV1, RuntimeAssetManifestV3 } from "./animated-image-manifest";

export function validMotionProfile(): MotionProfileV1 {
  return {
    profileVersion: 1,
    engineProfile: "life-v1",
    alphaBounds: { left: 0.1, top: 0.05, right: 0.9, bottom: 0.96 },
    breathZone: { left: 0.2, top: 0.5, right: 0.8, bottom: 0.84 },
    swayPivot: { x: 0.5, y: 0.72 },
  };
}

export function validAnimatedManifest(): RuntimeAssetManifestV3 {
  return {
    schemaVersion: 3,
    renderer: "animated-image-v1",
    petId: "pet-user-1",
    variantId: "variant-1",
    image: "body.png",
    motionProfile: "motion-profile.json",
    files: [
      { role: "main", relativePath: "body.png", sha256: "a".repeat(64) },
      { role: "motion-profile", relativePath: "motion-profile.json", sha256: "b".repeat(64) },
    ],
  };
}
