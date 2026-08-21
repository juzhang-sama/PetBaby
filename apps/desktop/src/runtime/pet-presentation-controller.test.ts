import { describe, expect, it, vi } from "vitest";
import type { PetRenderer } from "./pet-renderer";
import { PetPresentationController, type PetPresentationPorts } from "./pet-presentation-controller";
import { DEFAULT_PET_CALIBRATION } from "./pet-calibration";

function fakePorts(): PetPresentationPorts {
  return {
    renderer: {
      load: vi.fn(async () => undefined),
      resize: vi.fn(),
      playMotion: vi.fn(() => ({ cancel: vi.fn() })),
      setExpression: vi.fn(),
      setLookTarget: vi.fn(),
      setLipSync: vi.fn(),
      setCalibration: vi.fn(),
      hitTest: vi.fn(() => null),
      setVisibility: vi.fn(),
      update: vi.fn(),
      destroy: vi.fn(),
    } satisfies PetRenderer,
    effects: { play: vi.fn() },
    windowMotion: { shake: vi.fn(), bounce: vi.fn() },
    scheduler: { setTier: vi.fn() },
  };
}

describe("PetPresentationController", () => {
  it("does not retain a handle when a cat motion completes synchronously", () => {
    const ports = fakePorts();
    const cancel = vi.fn();
    ports.renderer.playCatMotion = vi.fn((_motion, _options, onFinished) => {
      onFinished?.();
      return { cancel };
    });
    const controller = new PetPresentationController(ports);
    const completed = vi.fn();

    controller.dispatchCatMotion([
      {
        type: "play",
        token: 7,
        motion: "pet-happy",
        priority: 90,
        loop: false,
        fadeInMs: 180,
        fadeOutMs: 140,
      },
    ], completed);
    controller.cancelCatMotions();

    expect(completed).toHaveBeenCalledWith(7);
    expect(cancel).not.toHaveBeenCalled();
  });

  it("maps a happy reaction to expression, motion, particles and window shake", () => {
    const ports = fakePorts();
    const controller = new PetPresentationController(ports);

    controller.dispatch({ type: "react-happy" });

    expect(ports.renderer.setExpression).toHaveBeenCalledWith("happy");
    expect(ports.renderer.playMotion).toHaveBeenCalledWith("react-happy", { priority: 60 });
    expect(ports.effects.play).toHaveBeenCalledWith("hearts", { opacity: 0.6, intensity: 0.6 });
    expect(ports.windowMotion.shake).toHaveBeenCalledWith({ amplitude: 2.4, durationMs: 180 });
    expect(ports.scheduler.setTier).toHaveBeenCalledWith("active");
  });

  it.each([
    ["front", null, "idle"],
    ["left", { x: -1, y: 0 }, "look-left"],
    ["right", { x: 1, y: 0 }, "look-right"],
  ] as const)("maps the %s look target without renderer-specific parameters", (target, point, motion) => {
    const ports = fakePorts();
    const controller = new PetPresentationController(ports);

    controller.dispatch({ type: "look", target });

    expect(ports.renderer.setLookTarget).toHaveBeenCalledWith(point);
    expect(ports.renderer.playMotion).toHaveBeenCalledWith(motion, { priority: 20 });
  });

  it("uses carried priority above direct reactions", () => {
    const ports = fakePorts();
    const controller = new PetPresentationController(ports);

    controller.dispatch({ type: "carried" });

    expect(ports.renderer.playMotion).toHaveBeenCalledWith("carried", { priority: 80, loop: true });
    expect(ports.scheduler.setTier).toHaveBeenCalledWith("active");
  });

  it("scales happy click feedback without changing behavior selection", () => {
    const ports = fakePorts();
    const controller = new PetPresentationController(ports);
    controller.setCalibration({ ...DEFAULT_PET_CALIBRATION, feedbackStrength: 0.25 });

    controller.dispatch({ type: "react-happy" });

    expect(ports.renderer.setExpression).toHaveBeenCalledWith("happy");
    expect(ports.renderer.playMotion).toHaveBeenCalledWith("react-happy", { priority: 60 });
    expect(ports.effects.play).toHaveBeenCalledWith("hearts", { opacity: 0.25, intensity: 0.25 });
    expect(ports.windowMotion.shake).toHaveBeenCalledWith({ amplitude: 1, durationMs: 180 });
    expect(ports.scheduler.setTier).toHaveBeenCalledWith("active");
  });

  it("keeps behavior and cooldown tier while suppressing zero-strength visual feedback", () => {
    const ports = fakePorts();
    const controller = new PetPresentationController(ports);
    controller.setCalibration({ ...DEFAULT_PET_CALIBRATION, feedbackStrength: 0 });

    controller.dispatch({ type: "react-happy" });

    expect(ports.renderer.setExpression).toHaveBeenCalledWith("happy");
    expect(ports.renderer.playMotion).toHaveBeenCalledWith("react-happy", { priority: 60 });
    expect(ports.effects.play).not.toHaveBeenCalled();
    expect(ports.windowMotion.shake).not.toHaveBeenCalled();
    expect(ports.scheduler.setTier).toHaveBeenCalledWith("active");
  });

  it("scales curious particles and landed bounce through the same calibration", () => {
    const ports = fakePorts();
    const controller = new PetPresentationController(ports);
    controller.setCalibration({ ...DEFAULT_PET_CALIBRATION, feedbackStrength: 0.25 });

    controller.dispatch({ type: "react-curious" });
    controller.dispatch({ type: "landed" });

    expect(ports.effects.play).toHaveBeenNthCalledWith(
      1,
      "sparkles",
      { opacity: 0.25, intensity: 0.25 },
    );
    expect(ports.effects.play).toHaveBeenNthCalledWith(
      2,
      "landing",
      { opacity: 0.25, intensity: 0.25 },
    );
    expect(ports.windowMotion.bounce).toHaveBeenCalledWith({ amplitude: 2, durationMs: 240 });
    expect(ports.renderer.playMotion).toHaveBeenCalledWith("react-curious", { priority: 60 });
    expect(ports.renderer.playMotion).toHaveBeenCalledWith("landed", { priority: 80 });
    expect(ports.scheduler.setTier).toHaveBeenCalledTimes(2);
  });
});
