import { describe, expect, it } from "vitest";
import {
  CatAutomationController,
  sampleBlinkEye,
  type CatAutomationMode,
} from "./cat-automation";

describe("cat automation", () => {
  it("keeps both eyelids continuous at the start and end while allowing independent closure", () => {
    expect(sampleBlinkEye(0, 0)).toBe(1);
    expect(sampleBlinkEye(1, 0)).toBe(1);
    expect(sampleBlinkEye(0, -0.12)).toBe(1);
    expect(sampleBlinkEye(1, -0.12)).toBe(1);
    expect(sampleBlinkEye(0, 0.12)).toBe(1);
    expect(sampleBlinkEye(1, 0.12)).toBe(1);
    expect(sampleBlinkEye(0.5, 0)).toBe(0);
    expect(sampleBlinkEye(0.5, 0.12)).toBeGreaterThan(0);
  });

  it("keeps low-frequency ear and tail automation inside authored bounds", () => {
    const controller = new CatAutomationController({ random: () => 0.25 });
    const samples = Array.from({ length: 600 }, () => controller.update(16, "idle"));

    expect(samples.every((frame) => frame.earLeft >= -0.35 && frame.earLeft <= 0.35)).toBe(true);
    expect(samples.every((frame) => frame.earRight >= -0.35 && frame.earRight <= 0.35)).toBe(true);
    expect(samples.every((frame) => frame.tailAngle >= -12 && frame.tailAngle <= 12)).toBe(true);
    expect(samples.every((frame) => frame.tailCurl >= -0.45 && frame.tailCurl <= 0.45)).toBe(true);
    expect(new Set(samples.map((frame) => frame.tailAngle.toFixed(3))).size).toBeGreaterThan(10);
  });

  it("only amplifies automation in pointer focus and suppresses it while dragging", () => {
    const frameFor = (mode: CatAutomationMode) => {
      const controller = new CatAutomationController({ random: () => 0.25 });
      return controller.update(2_000, mode);
    };
    const idle = frameFor("idle");
    const focused = frameFor("pointerFocus");
    const dragging = frameFor("dragging");

    expect(Math.abs(focused.tailAngle)).toBeGreaterThan(Math.abs(idle.tailAngle));
    expect(Math.abs(focused.earLeft)).toBeGreaterThan(Math.abs(idle.earLeft));
    expect(Math.abs(dragging.tailAngle)).toBeLessThan(Math.abs(idle.tailAngle));
    expect(Math.abs(dragging.earLeft)).toBeLessThan(Math.abs(idle.earLeft));
    expect(focused.eyeLeftOpen).toBe(idle.eyeLeftOpen);
    expect(focused.eyeRightOpen).toBe(idle.eyeRightOpen);
  });

  it("does not advance or catch up while paused", () => {
    const controller = new CatAutomationController({ random: () => 0.25 });
    const before = controller.update(1_000, "idle");
    const paused = controller.update(60_000, "paused");
    const resumed = controller.update(0, "idle");

    expect(paused).toEqual(before);
    expect(resumed).toEqual(before);
  });

  it("starts open instead of blinking immediately on mount", () => {
    const controller = new CatAutomationController({ random: () => 0.25 });
    const firstFrame = controller.update(16, "idle");

    expect(firstFrame.eyeLeftOpen).toBe(1);
    expect(firstFrame.eyeRightOpen).toBe(1);
  });
});
