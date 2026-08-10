import { describe, expect, it, vi } from "vitest";
import { installedPetAssetUrl } from "./pet-asset-url";

describe("installedPetAssetUrl", () => {
  it("uses Tauri's custom-protocol converter for an installed pet asset", () => {
    const converter = vi.fn(() => "http://pet-asset.localhost/pet-user-1/assets/body.png");

    expect(installedPetAssetUrl("pet-user-1", "body.png", converter)).toBe(
      "http://pet-asset.localhost/pet-user-1/assets/body.png",
    );
    expect(converter).toHaveBeenCalledWith("pet-user-1/assets/body.png", "pet-asset");
  });

  it("normalizes asset separators and rejects traversal", () => {
    const converter = vi.fn(() => "converted");

    expect(installedPetAssetUrl("pet-user-1", "layers\\body.png", converter)).toBe("converted");
    expect(converter).toHaveBeenCalledWith("pet-user-1/assets/layers/body.png", "pet-asset");
    expect(() => installedPetAssetUrl("pet-user-1", "../body.png", converter)).toThrow(
      "unsafe asset path",
    );
  });
});
