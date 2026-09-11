import { describe, expect, it } from "vitest";
import { parseFrameSequenceManifest } from "./frame-sequence-manifest";
import { validV6Manifest } from "./frame-sequence-test-fixtures";

describe("parseFrameSequenceManifest", () => {
  it("parses a valid v6 manifest and normalizes backslashes", () => {
    const manifest = validV6Manifest();
    manifest.baseImage = "body\\body.png";
    const parsed = parseFrameSequenceManifest(manifest);
    expect(parsed.baseImage).toBe("body/body.png");
    expect(parsed.actions).toHaveLength(2);
    expect(parsed.defaultAction).toBe("breath");
    expect(parsed.semantics.idle).toBe("breath");
    expect(parsed.idleSchedule?.entries[0]?.minIntervalMs).toBe(2500);
    expect(parsed.idleSchedule?.entries[0]?.maxIntervalMs).toBe(6000);
  });

  it("rejects an unsupported schemaVersion", () => {
    const manifest = validV6Manifest();
    expect(() => parseFrameSequenceManifest({ ...manifest, schemaVersion: 5 })).toThrow(/unsupported schemaVersion/);
  });

  it("rejects an unsupported renderer", () => {
    const manifest = validV6Manifest();
    expect(() => parseFrameSequenceManifest({ ...manifest, renderer: "animated-image-v1" })).toThrow(/unsupported renderer/);
  });

  it("rejects an unknown species", () => {
    const manifest = validV6Manifest();
    expect(() => parseFrameSequenceManifest({ ...manifest, species: "bird" })).toThrow(/species/);
  });

  it("rejects a non-PNG baseImage", () => {
    const manifest = validV6Manifest();
    expect(() => parseFrameSequenceManifest({ ...manifest, baseImage: "body.jpg" })).toThrow(/PNG/);
  });

  it("accepts WebP frame and base image paths", () => {
    const manifest = validV6Manifest();
    manifest.baseImage = "body.webp";
    manifest.actions = [
      { ...manifest.actions[0]!, frames: ["actions/breath/f00.webp", "actions/breath/f01.webp"] },
      manifest.actions[1]!,
    ];
    manifest.files = [
      { role: "base", relativePath: "body.webp", sha256: "a".repeat(64) },
      { role: "frame", relativePath: "actions/breath/f00.webp", sha256: "b".repeat(64) },
      { role: "frame", relativePath: "actions/breath/f01.webp", sha256: "c".repeat(64) },
      { role: "frame", relativePath: "actions/blink/f00.png", sha256: "d".repeat(64) },
      { role: "frame", relativePath: "actions/blink/f01.png", sha256: "e".repeat(64) },
    ];
    const parsed = parseFrameSequenceManifest(manifest);
    expect(parsed.baseImage).toBe("body.webp");
    expect(parsed.actions[0]?.frames).toEqual(["actions/breath/f00.webp", "actions/breath/f01.webp"]);
  });

  it("rejects an anchorPolicy other than fixed", () => {
    const manifest = validV6Manifest();
    expect(() => parseFrameSequenceManifest({ ...manifest, anchorPolicy: "floating" })).toThrow(/anchorPolicy/);
  });

  it("rejects a defaultAction that is not declared", () => {
    const manifest = validV6Manifest();
    expect(() => parseFrameSequenceManifest({ ...manifest, defaultAction: "sleep" })).toThrow(/defaultAction/);
  });

  it("rejects duplicate actionIds", () => {
    const manifest = validV6Manifest();
    manifest.actions = [
      { ...manifest.actions[0]! },
      { ...manifest.actions[0]! },
    ];
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/duplicate actionId/);
  });

  it("rejects a non-positive frameDurationMs", () => {
    const manifest = validV6Manifest();
    manifest.actions = [{ ...manifest.actions[0]!, frameDurationMs: 0 }];
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/frameDurationMs/);
  });

  it("rejects empty frames arrays", () => {
    const manifest = validV6Manifest();
    manifest.actions = [{ ...manifest.actions[0]!, frames: [] }];
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/frames/);
  });

  it("rejects duplicate frame paths", () => {
    const manifest = validV6Manifest();
    manifest.actions = [{
      ...manifest.actions[0]!,
      frames: ["actions/breath/f00.png", "actions/breath/f00.png"],
    }];
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/duplicate frame path/);
  });

  it("rejects semantics referencing an unknown action", () => {
    const manifest = validV6Manifest();
    manifest.semantics = { idle: "breath", "react-happy": "tail-wag-unknown" };
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/unknown action/);
  });

  it("rejects semantics that do not declare every product motion", () => {
    const manifest = validV6Manifest();
    manifest.semantics = { idle: "breath" };
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(
      /every product motion: missing look-left, look-right, react-happy, react-curious, carried, landed, sleep, wake/,
    );
  });

  it("rejects an inverted hitBounds", () => {
    const manifest = validV6Manifest();
    manifest.hitBounds = { left: 0.9, top: 0.1, right: 0.1, bottom: 0.9 };
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/inverted rect/);
  });

  it("rejects blink intervals where max < min", () => {
    const manifest = validV6Manifest();
    manifest.blink = { enabled: true, minIntervalMs: 6000, maxIntervalMs: 2500 };
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/blink/);
  });

  it("rejects file entries with an invalid sha256", () => {
    const manifest = validV6Manifest();
    manifest.files = [{ ...manifest.files[0]!, sha256: "not-a-hash" }];
    expect(() => parseFrameSequenceManifest(manifest)).toThrow(/sha256/);
  });

  it("accepts a manifest without blink and hitBounds", () => {
    const manifest = validV6Manifest();
    manifest.blink = undefined;
    manifest.hitBounds = undefined;
    const parsed = parseFrameSequenceManifest(manifest);
    expect(parsed.idleSchedule).toBeUndefined();
    expect(parsed.hitBounds).toBeUndefined();
  });

  it("parses a valid v7 manifest with idleSchedule", () => {
    const manifest = validV6Manifest() as unknown as Record<string, unknown>;
    manifest.schemaVersion = 7;
    manifest.blink = undefined;
    manifest.idleSchedule = {
      entries: [
        { actionId: "blink", weight: 1, minIntervalMs: 2500, maxIntervalMs: 6000 },
        { actionId: "ear-twitch", weight: 0.5, minIntervalMs: 4000, maxIntervalMs: 8000 },
      ],
    };
    const parsed = parseFrameSequenceManifest(manifest);
    expect(parsed.schemaVersion).toBe(7);
    expect(parsed.idleSchedule?.entries).toHaveLength(2);
    expect(parsed.idleSchedule?.entries[1]?.actionId).toBe("ear-twitch");
  });
});
