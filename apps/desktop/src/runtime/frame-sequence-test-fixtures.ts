import type { RuntimeAssetManifestV6 } from "./frame-sequence-manifest";

export function validV6Manifest(): RuntimeAssetManifestV6 {
  return {
    schemaVersion: 6,
    renderer: "frame-sequence-v1",
    petId: "01-longhair-black-white",
    variantId: "pixel-mid-simple-v1",
    displayName: "长毛黑白猫",
    species: "cat",
    baseImage: "body.png",
    defaultAction: "breath",
    anchorPolicy: "fixed",
    actions: [
      {
        actionId: "breath",
        loop: true,
        frameDurationMs: 180,
        frames: ["actions/breath/f00.png", "actions/breath/f01.png"],
      },
      {
        actionId: "blink",
        loop: true,
        frameDurationMs: 150,
        frames: ["actions/blink/f00.png", "actions/blink/f01.png"],
      },
    ],
    semantics: { idle: "breath" },
    blink: { enabled: true, minIntervalMs: 2500, maxIntervalMs: 6000 },
    hitBounds: { left: 0.05, top: 0.08, right: 0.95, bottom: 0.98 },
    files: [
      { role: "base", relativePath: "body.png", sha256: "a".repeat(64) },
      { role: "frame", relativePath: "actions/breath/f00.png", sha256: "b".repeat(64) },
      { role: "frame", relativePath: "actions/breath/f01.png", sha256: "c".repeat(64) },
      { role: "frame", relativePath: "actions/blink/f00.png", sha256: "d".repeat(64) },
      { role: "frame", relativePath: "actions/blink/f01.png", sha256: "e".repeat(64) },
    ],
  };
}
