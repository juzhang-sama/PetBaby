import { describe, expect, it } from "vitest";
import {
  chaseStep,
  DEFAULT_PHYSICS_CONFIG,
  edgeStrollStep,
  stepPhysics,
  throwPet,
  type PetPhysicsState,
  type PhysicsBounds,
  type PhysicsConfig,
} from "./pet-physics";

const BOUNDS: PhysicsBounds = { left: 0, top: 0, right: 1_000, bottom: 700 };

function idleState(x = 100, y = 700): PetPhysicsState {
  return { x, y, vx: 0, vy: 0, mode: "idle", direction: 1 };
}

describe("throwPet", () => {
  it("puts the pet into falling mode with the given velocity", () => {
    const thrown = throwPet(idleState(), 300, -500);
    expect(thrown.mode).toBe("falling");
    expect(thrown.vx).toBe(300);
    expect(thrown.vy).toBe(-500);
  });
});

describe("stepPhysics", () => {
  const config: PhysicsConfig = {
    ...DEFAULT_PHYSICS_CONFIG,
    gravity: 1_000,
    airDrag: 0,
    settleSpeed: 100,
    bounceFactor: 0.5,
  };

  it("accelerates a falling pet downward", () => {
    const state: PetPhysicsState = { ...idleState(100, 500), mode: "falling", vx: 0, vy: 0 };
    const next = stepPhysics(state, 200, BOUNDS, config);
    expect(next.mode).toBe("falling");
    expect(next.vy).toBeGreaterThan(0);
    expect(next.y).toBeGreaterThan(500);
  });

  it("clamps horizontal motion to the bounds", () => {
    const state: PetPhysicsState = { ...idleState(100, 500), mode: "falling", vx: -5_000, vy: 0 };
    const next = stepPhysics(state, 200, BOUNDS, config);
    expect(next.x).toBe(BOUNDS.left);
  });

  it("bounces off the floor when the impact speed is high", () => {
    const state: PetPhysicsState = {
      ...idleState(100, 690),
      mode: "falling",
      vx: 0,
      vy: 500,
    };
    const next = stepPhysics(state, 100, BOUNDS, config);
    expect(next.y).toBe(BOUNDS.bottom);
    expect(next.mode).toBe("falling");
    expect(next.vy).toBeLessThan(0);
  });

  it("settles to idle when the impact speed is low", () => {
    const state: PetPhysicsState = {
      ...idleState(100, 690),
      mode: "falling",
      vx: 0,
      vy: 0,
    };
    const next = stepPhysics(state, 100, BOUNDS, config);
    expect(next.mode).toBe("idle");
    expect(next.vy).toBe(0);
    expect(next.y).toBe(BOUNDS.bottom);
  });

  it("ignores non-falling states", () => {
    const state = idleState();
    const next = stepPhysics(state, 200, BOUNDS, config);
    expect(next).toEqual(state);
  });
});

describe("edgeStrollStep", () => {
  it("moves along the floor in the current direction", () => {
    const next = edgeStrollStep(
      { ...idleState(100, 700), mode: "strolling", direction: 1 },
      200,
      BOUNDS,
      DEFAULT_PHYSICS_CONFIG,
    );
    expect(next.x).toBeGreaterThan(100);
    expect(next.y).toBe(BOUNDS.bottom);
    expect(next.mode).toBe("strolling");
  });

  it("bounces off the right edge and reverses direction", () => {
    const next = edgeStrollStep(
      { ...idleState(990, 700), mode: "strolling", direction: 1 },
      500,
      BOUNDS,
      DEFAULT_PHYSICS_CONFIG,
    );
    expect(next.x).toBe(BOUNDS.right);
    expect(next.direction).toBe(-1);
  });

  it("bounces off the left edge and reverses direction", () => {
    const next = edgeStrollStep(
      { ...idleState(10, 700), mode: "strolling", direction: -1 },
      500,
      BOUNDS,
      DEFAULT_PHYSICS_CONFIG,
    );
    expect(next.x).toBe(BOUNDS.left);
    expect(next.direction).toBe(1);
  });
});

describe("chaseStep", () => {
  it("moves toward the target and stops when reached", () => {
    const first = chaseStep(
      { ...idleState(100, 700), mode: "chasing" },
      200,
      300,
      BOUNDS,
      DEFAULT_PHYSICS_CONFIG,
    );
    expect(first.mode).toBe("chasing");
    expect(first.x).toBeGreaterThan(100);
    expect(first.x).toBeLessThanOrEqual(300);

    const reached = chaseStep(
      { ...idleState(290, 700), mode: "chasing" },
      200,
      300,
      BOUNDS,
      DEFAULT_PHYSICS_CONFIG,
    );
    expect(reached.mode).toBe("idle");
    expect(reached.x).toBe(300);
  });

  it("clamps the target to the bounds", () => {
    const next = chaseStep(
      { ...idleState(100, 700), mode: "chasing" },
      100_000,
      99_999,
      BOUNDS,
      DEFAULT_PHYSICS_CONFIG,
    );
    expect(next.x).toBe(BOUNDS.right);
  });
});
