import { describe, expect, it } from "vitest";
import { AnimStateMachine, type AnimStateDef } from "./anim-machine";

const states: AnimStateDef[] = [
  {
    id: "idle",
    clip: {
      name: "idle",
      durationMs: 1_000,
      loop: true,
      tracks: { tilt: [{ timeMs: 0, value: 0 }] },
    },
  },
  {
    id: "look-left",
    clip: {
      name: "look-left",
      durationMs: 200,
      tracks: {
        tilt: [
          { timeMs: 0, value: 0 },
          { timeMs: 100, value: 6 },
          { timeMs: 200, value: 0 },
        ],
      },
    },
    followUp: "idle",
    blendMs: 50,
  },
  {
    id: "happy",
    clip: {
      name: "happy",
      durationMs: 100,
      tracks: {
        accent: [
          { timeMs: 0, value: 0 },
          { timeMs: 50, value: 1 },
          { timeMs: 100, value: 0 },
        ],
      },
    },
    followUp: "idle",
  },
];

describe("AnimStateMachine", () => {
  it("plays the initial state", () => {
    const machine = new AnimStateMachine(states, "idle");
    machine.update(0);
    expect(machine.params().tilt).toBe(0);
  });

  it("starts a new state at its beginning", () => {
    const machine = new AnimStateMachine(states, "idle");
    machine.update(0);
    machine.play("look-left", 1_000, 0);
    machine.update(1_100);
    expect(machine.params().tilt).toBeCloseTo(6, 0);
  });

  it("crossfades between states", () => {
    const machine = new AnimStateMachine(states, "idle");
    machine.update(0);
    machine.play("happy", 1_000, 100);
    machine.update(1_050);
    const params = machine.params();
    expect(params.accent).toBeGreaterThan(0.1);
    expect(params.accent).toBeLessThan(0.9);
  });

  it("follows up to the next state after a one-shot finishes", () => {
    const machine = new AnimStateMachine(states, "idle");
    machine.update(0);
    machine.play("happy", 1_000, 0);
    machine.update(1_100);
    expect(machine.stateId).toBe("idle");
    expect(machine.params().accent).toBeCloseTo(0, 1);
  });

  it("ignores replaying the current state", () => {
    const machine = new AnimStateMachine(states, "idle");
    machine.update(0);
    machine.play("idle", 500, 0);
    expect(machine.stateId).toBe("idle");
  });

  it("throws for an unknown state", () => {
    const machine = new AnimStateMachine(states, "idle");
    expect(() => machine.play("nope", 0)).toThrow();
  });
});
