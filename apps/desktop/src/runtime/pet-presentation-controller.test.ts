import { describe, expect, it, vi } from "vitest";
import type { PetRenderer } from "./pet-renderer";
import { PetPresentationController, type PetPresentationPorts } from "./pet-presentation-controller";

function fakePorts(): PetPresentationPorts {
  return {
    renderer: {
      load: vi.fn(async () => undefined),
      resize: vi.fn(),
      playMotion: vi.fn(() => ({ cancel: vi.fn() })),
      setExpression: vi.fn(),
      setLookTarget: vi.fn(),
      setLipSync: vi.fn(),
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
  it("maps a happy reaction to expression, motion, particles and window shake", () => {
    const ports = fakePorts();
    const controller = new PetPresentationController(ports);

    controller.dispatch({ type: "react-happy" });

    expect(ports.renderer.setExpression).toHaveBeenCalledWith("happy");
    expect(ports.renderer.playMotion).toHaveBeenCalledWith("react-happy", { priority: 60 });
    expect(ports.effects.play).toHaveBeenCalledWith("hearts");
    expect(ports.windowMotion.shake).toHaveBeenCalledWith({ amplitude: 4, durationMs: 180 });
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
});
