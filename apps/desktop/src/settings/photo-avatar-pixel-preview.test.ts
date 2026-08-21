import { describe, expect, it } from "vitest";
import { photoAvatarPreviewViewport } from "./photo-avatar-pixel-preview";

describe("photo avatar pixel preview viewport", () => {
  it("uses a stable runtime-check size while the preview container is hidden", () => {
    const root = {
      getBoundingClientRect: () => ({ width: 0, height: 0 }),
    } as HTMLElement;

    expect(photoAvatarPreviewViewport(root, 1.5)).toEqual({
      width: 480,
      height: 300,
      dpr: 1.5,
    });
  });

  it("uses the measured size after the preview container becomes visible", () => {
    const root = {
      clientWidth: 426,
      clientHeight: 319,
      getBoundingClientRect: () => ({ width: 426.4, height: 318.6 }),
    } as HTMLElement;

    expect(photoAvatarPreviewViewport(root, 2)).toEqual({
      width: 426,
      height: 319,
      dpr: 2,
    });
  });

  it("uses the content box so resize observation does not feed border size back into the canvas", () => {
    const root = {
      clientWidth: 480,
      clientHeight: 300,
      getBoundingClientRect: () => ({ width: 482, height: 302 }),
    } as HTMLElement;

    expect(photoAvatarPreviewViewport(root, 1)).toEqual({
      width: 480,
      height: 300,
      dpr: 1,
    });
  });
});
