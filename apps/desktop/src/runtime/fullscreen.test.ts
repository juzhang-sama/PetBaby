import { describe, expect, it } from "vitest";
import { classifyFullscreen } from "./fullscreen";

describe("classifyFullscreen", () => {
  const monitor = { left: 0, top: 0, right: 1920, bottom: 1080 };

  it("accepts a borderless window within two pixels", () => {
    expect(classifyFullscreen({ left: -1, top: 0, right: 1921, bottom: 1080 }, monitor, 2)).toBe(true);
  });

  it("rejects a maximized work-area window that leaves the taskbar visible", () => {
    expect(classifyFullscreen({ left: 0, top: 0, right: 1920, bottom: 1040 }, monitor, 2)).toBe(false);
  });
});
