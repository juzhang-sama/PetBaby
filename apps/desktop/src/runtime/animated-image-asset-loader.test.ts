import { describe, expect, it, vi } from "vitest";
import { loadAnimatedImageAsset } from "./animated-image-asset-loader";
import { validAnimatedManifest, validMotionProfile } from "./animated-image-test-fixtures";

describe("loadAnimatedImageAsset", () => {
  const assetUrl = (petId: string, path: string) => `http://pet-asset.localhost/${petId}/assets/${path}`;

  it("loads a parsed motion profile alongside the image URL", async () => {
    const fetchJson = vi.fn(async () => validMotionProfile());

    await expect(loadAnimatedImageAsset("pet-user-1", validAnimatedManifest(), assetUrl, fetchJson))
      .resolves.toEqual({
        kind: "animated-image",
        imageUrl: "http://pet-asset.localhost/pet-user-1/assets/body.png",
        motionProfile: validMotionProfile(),
      });
    expect(fetchJson).toHaveBeenCalledWith("http://pet-asset.localhost/pet-user-1/assets/motion-profile.json");
  });

  it("propagates a missing motion profile instead of returning a static asset", async () => {
    const fetchJson = vi.fn(async () => { throw new Error("HTTP 404"); });
    await expect(loadAnimatedImageAsset(
      "pet-user-1",
      validAnimatedManifest(),
      assetUrl,
      fetchJson,
    )).rejects.toThrow("HTTP 404");
  });

  it("rejects an invalid fetched motion profile", async () => {
    const invalid = validMotionProfile();
    invalid.swayPivot.y = 0.99;

    await expect(loadAnimatedImageAsset("pet-user-1", validAnimatedManifest(), assetUrl, async () => invalid))
      .rejects.toThrow(/outside alpha/i);
  });
});
