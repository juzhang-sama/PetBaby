import { describe, expect, it } from "vitest";
import { isLive2DPreviewMode, resolvePreviewUrl } from "./preview";

describe("Live2D preview", () => {
  it("requires an explicit preview flag", () => {
    expect(isLive2DPreviewMode("?live2dPreview=1")).toBe(true);
    expect(isLive2DPreviewMode("?live2dPreview=0")).toBe(false);
    expect(isLive2DPreviewMode("")).toBe(false);
  });

  it("resolves package resources relative to the manifest", () => {
    expect(resolvePreviewUrl(
      "/builtin-pets/pet-live2d-v1/manifest.json",
      "pet-live2d-v1.model3.json",
      "http://127.0.0.1:1420",
    )).toBe("http://127.0.0.1:1420/builtin-pets/pet-live2d-v1/pet-live2d-v1.model3.json");
  });
});
