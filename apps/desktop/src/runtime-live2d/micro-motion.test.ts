import { describe, expect, it } from "vitest";
import { MicroMotionController } from "./micro-motion";

describe("MicroMotionController", () => {
  it("starts neutral and stays inside the production ranges", () => {
    const controller = new MicroMotionController();

    expect(controller.update(0)).toEqual({ breath: 0.5, bodySway: 0 });
    for (let index = 0; index < 200; index += 1) {
      const frame = controller.update(100);
      expect(frame.breath).toBeGreaterThanOrEqual(0);
      expect(frame.breath).toBeLessThanOrEqual(1);
      expect(Math.abs(frame.bodySway)).toBeLessThanOrEqual(6);
    }
  });

  it("freezes the exact frame and elapsed time while paused", () => {
    const paused = new MicroMotionController();
    const control = new MicroMotionController();
    for (let index = 0; index < 7; index += 1) {
      paused.update(100);
      control.update(100);
    }
    const beforePause = paused.update(0);

    paused.setPaused(true);
    expect(paused.update(5_000)).toBe(beforePause);
    paused.setPaused(false);

    expect(paused.update(100)).toEqual(control.update(100));
  });

  it("suppresses sway and weakens breath while carried", () => {
    const controller = new MicroMotionController();
    controller.setCarried(true);

    for (let index = 0; index < 50; index += 1) {
      const frame = controller.update(100);
      expect(frame.bodySway).toBe(0);
      expect(Math.abs(frame.breath - 0.5)).toBeLessThanOrEqual(0.125);
    }
  });
});
