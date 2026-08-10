import { describe, expect, it } from "vitest";
import { parseAnimatedImageManifest, parseMotionProfile } from "./animated-image-manifest";
import { validAnimatedManifest, validMotionProfile } from "./animated-image-test-fixtures";

describe("animated image manifest v3", () => {
  it("parses a valid animated image manifest and motion profile", () => {
    expect(parseAnimatedImageManifest(validAnimatedManifest())).toMatchObject({
      schemaVersion: 3,
      renderer: "animated-image-v1",
      image: "body.png",
      motionProfile: "motion-profile.json",
    });
    expect(parseMotionProfile(validMotionProfile())).toMatchObject({
      profileVersion: 1,
      engineProfile: "life-v1",
    });
  });

  it("rejects unknown profile versions and engines", () => {
    const version = validMotionProfile();
    version.profileVersion = 2 as 1;
    expect(() => parseMotionProfile(version)).toThrow(/unknown profile version/i);

    const engine = validMotionProfile();
    engine.engineProfile = "other" as "life-v1";
    expect(() => parseMotionProfile(engine)).toThrow(/unknown engine/i);
  });

  it("rejects non-finite, out-of-range and inverted profile geometry", () => {
    const nonFinite = validMotionProfile();
    nonFinite.alphaBounds.left = Number.NaN;
    expect(() => parseMotionProfile(nonFinite)).toThrow(/non-finite/i);

    const outOfRange = validMotionProfile();
    outOfRange.alphaBounds.right = 1.1;
    expect(() => parseMotionProfile(outOfRange)).toThrow(/out of range/i);

    const inverted = validMotionProfile();
    inverted.alphaBounds.right = inverted.alphaBounds.left;
    expect(() => parseMotionProfile(inverted)).toThrow(/inverted rect/i);
  });

  it("rejects a breath zone outside alpha bounds", () => {
    const profile = validMotionProfile();
    profile.breathZone.right = 0.95;
    expect(() => parseMotionProfile(profile)).toThrow(/outside alpha/i);
  });

  it("rejects a breath zone above the face safety line", () => {
    const profile = validMotionProfile();
    profile.breathZone.top = 0.1;
    expect(() => parseMotionProfile(profile)).toThrow(/face safety/i);
  });

  it("rejects a sway pivot outside alpha bounds", () => {
    const profile = validMotionProfile();
    profile.swayPivot.x = 0.95;
    expect(() => parseMotionProfile(profile)).toThrow(/outside alpha/i);
  });

  it("rejects traversal in animated asset paths", () => {
    const manifest = validAnimatedManifest();
    manifest.motionProfile = "../motion-profile.json";
    expect(() => parseAnimatedImageManifest(manifest)).toThrow(/relative path/i);
  });

  it("requires safe PNG and JSON entries with matching roles", () => {
    const wrongImage = validAnimatedManifest();
    wrongImage.image = "body.jpg";
    expect(() => parseAnimatedImageManifest(wrongImage)).toThrow(/PNG/i);

    const wrongProfile = validAnimatedManifest();
    wrongProfile.motionProfile = "motion-profile.txt";
    expect(() => parseAnimatedImageManifest(wrongProfile)).toThrow(/JSON/i);

    const missingMain = validAnimatedManifest();
    missingMain.files[0]!.role = "thumbnail";
    expect(() => parseAnimatedImageManifest(missingMain)).toThrow(/main file/i);

    const missingProfile = validAnimatedManifest();
    missingProfile.files[1]!.role = "metadata";
    expect(() => parseAnimatedImageManifest(missingProfile)).toThrow(/motion-profile file/i);
  });

  it("rejects unknown manifest versions and renderers", () => {
    const version = validAnimatedManifest();
    (version as { schemaVersion: number }).schemaVersion = 4;
    expect(() => parseAnimatedImageManifest(version)).toThrow(/schemaVersion/i);

    const renderer = validAnimatedManifest();
    (renderer as { renderer: string }).renderer = "static-png-v1";
    expect(() => parseAnimatedImageManifest(renderer)).toThrow(/renderer/i);
  });
});
