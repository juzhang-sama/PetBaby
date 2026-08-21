import { describe, expect, it } from "vitest";
import {
  initialCatMotionSchedulerState,
  scheduleCatMotion,
  type CatMotionSchedulerContext,
} from "./cat-motion-scheduler";

const context = (overrides: Partial<CatMotionSchedulerContext> = {}): CatMotionSchedulerContext => ({
  localHour: 12,
  random: () => 0,
  paused: false,
  ...overrides,
});

describe("scheduleCatMotion", () => {
  it("starts in a low-priority looping idle without taking over a user action", () => {
    const idle = scheduleCatMotion(initialCatMotionSchedulerState(), { type: "start" }, context());
    const pet = scheduleCatMotion(idle.state, { type: "pet" }, context());
    const ignoredIdle = scheduleCatMotion(pet.state, { type: "start" }, context());

    expect(idle.commands).toEqual([
      expect.objectContaining({ type: "play", motion: "breathing", priority: 10, loop: true }),
    ]);
    expect(pet.state.mode).toBe("petHappy");
    expect(ignoredIdle).toEqual({ state: pet.state, commands: [] });
  });

  it("lets petting interrupt an autonomous stretch and dragging interrupt everything", () => {
    const autonomous = scheduleCatMotion(
      initialCatMotionSchedulerState(),
      { type: "autonomous-due" },
      context({ random: () => 0.99 }),
    );
    const pet = scheduleCatMotion(autonomous.state, { type: "pet" }, context());
    const drag = scheduleCatMotion(pet.state, { type: "drag-start" }, context());

    expect(autonomous.commands).toEqual([
      expect.objectContaining({ type: "play", motion: "half-stand-stretch", priority: 30 }),
    ]);
    expect(pet.commands).toEqual([
      expect.objectContaining({ type: "cancel", token: autonomous.state.activeToken }),
      expect.objectContaining({ type: "play", motion: "pet-happy", priority: 90 }),
    ]);
    expect(drag.state.mode).toBe("dragging");
    expect(drag.commands).toEqual([
      expect.objectContaining({ type: "cancel", token: pet.state.activeToken }),
      expect.objectContaining({ type: "hold", priority: 100 }),
    ]);
  });

  it("returns pointer focus and matching completions smoothly to idle", () => {
    const focused = scheduleCatMotion(
      initialCatMotionSchedulerState(),
      { type: "pointer-enter" },
      context(),
    );
    const left = scheduleCatMotion(focused.state, { type: "pointer-leave" }, context());
    const completed = scheduleCatMotion(
      focused.state,
      { type: "motion-complete", token: focused.state.activeToken! },
      context(),
    );

    expect(focused.state.mode).toBe("pointerFocus");
    expect(left.state.mode).toBe("idle");
    expect(completed.state.mode).toBe("idle");
    for (const result of [focused, left, completed]) {
      for (const command of result.commands) {
        expect(command).toEqual(expect.objectContaining({ fadeInMs: expect.any(Number), fadeOutMs: expect.any(Number) }));
      }
    }
  });

  it("ignores a late completion from an interrupted action", () => {
    const autonomous = scheduleCatMotion(
      initialCatMotionSchedulerState(),
      { type: "autonomous-due" },
      context(),
    );
    const oldToken = autonomous.state.activeToken!;
    const pet = scheduleCatMotion(autonomous.state, { type: "pet" }, context());
    const stale = scheduleCatMotion(
      pet.state,
      { type: "motion-complete", token: oldToken },
      context(),
    );

    expect(stale).toEqual({ state: pet.state, commands: [] });
  });

  it("raises sleepy activity weights at night without forcing a sleepy action", () => {
    const daytime = scheduleCatMotion(
      initialCatMotionSchedulerState(),
      { type: "autonomous-due" },
      context({ localHour: 12, random: () => 0.4 }),
    );
    const nighttime = scheduleCatMotion(
      initialCatMotionSchedulerState(),
      { type: "autonomous-due" },
      context({ localHour: 23, random: () => 0.4 }),
    );
    const nightStillCanTwitch = scheduleCatMotion(
      initialCatMotionSchedulerState(),
      { type: "autonomous-due" },
      context({ localHour: 23, random: () => 0 }),
    );

    expect(daytime.commands[0]).toEqual(expect.objectContaining({ motion: "ear-twitch" }));
    expect(nighttime.commands[0]).toEqual(expect.objectContaining({ motion: "sleepy-yawn" }));
    expect(nightStillCanTwitch.commands[0]).toEqual(expect.objectContaining({ motion: "ear-twitch" }));
  });

  it("does not advance the autonomous timer while paused", () => {
    const state = { ...initialCatMotionSchedulerState(), autonomousElapsedMs: 29_000 };
    const paused = scheduleCatMotion(state, { type: "tick", elapsedMs: 5_000 }, context({ paused: true }));
    const resumed = scheduleCatMotion(paused.state, { type: "tick", elapsedMs: 1_000 }, context());

    expect(paused.state.autonomousElapsedMs).toBe(29_000);
    expect(paused.commands).toEqual([]);
    expect(resumed.state.mode).toBe("autonomous");
    expect(resumed.state.autonomousElapsedMs).toBe(0);
  });

  it("keeps the edge-hidden state until a priority-100 edge recall", () => {
    const hidden = scheduleCatMotion(
      initialCatMotionSchedulerState(),
      { type: "edge-hidden" },
      context(),
    );
    const ignoredPet = scheduleCatMotion(hidden.state, { type: "pet" }, context());
    const recalled = scheduleCatMotion(hidden.state, { type: "edge-recall" }, context());

    expect(hidden.state.mode).toBe("edgeHidden");
    expect(ignoredPet).toEqual({ state: hidden.state, commands: [] });
    expect(recalled.state.mode).toBe("idle");
    expect(recalled.commands).toEqual([
      expect.objectContaining({ type: "play", motion: "breathing", priority: 100, loop: false }),
    ]);

    const settled = scheduleCatMotion(
      recalled.state,
      { type: "motion-complete", token: recalled.state.activeToken! },
      context(),
    );
    expect(settled.commands).toEqual([
      expect.objectContaining({ type: "play", motion: "breathing", priority: 10, loop: true }),
    ]);
  });
});
