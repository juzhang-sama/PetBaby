import { describe, expect, it } from "vitest";
import { decide } from "./decision";

const baseState = {
  schemaVersion: 1 as const,
  petId: "pet-1",
  energy: 0.7,
  mood: 0.6,
  bond: 0.3,
  lastSeenAt: "2026-08-03T00:00:00.000Z",
  lastInteractionAt: "2026-08-03T00:00:00.000Z",
};

const emptyPolicy = { cooldowns: {} };

describe("decide", () => {
  it("maps head-clicked to a look intent", () => {
    const intents = decide({ event: { type: "head-clicked" }, state: baseState, policy: emptyPolicy });
    expect(intents).toContainEqual({ type: "look", target: "front" });
  });

  it("maps double-clicked to react-happy", () => {
    const intents = decide({ event: { type: "double-clicked" }, state: baseState, policy: emptyPolicy });
    expect(intents).toContainEqual({ type: "react-happy" });
  });

  it("maps drag-start to carried", () => {
    const intents = decide({ event: { type: "drag-start" }, state: baseState, policy: emptyPolicy });
    expect(intents).toContainEqual({ type: "carried" });
  });

  it("maps drag-end to landed", () => {
    const intents = decide({ event: { type: "drag-end" }, state: baseState, policy: emptyPolicy });
    expect(intents).toContainEqual({ type: "landed" });
  });

  it("emits a look intent after an idle period", () => {
    const intents = decide({ event: { type: "idle-tick", elapsedMs: 45_000 }, state: baseState, policy: emptyPolicy });
    expect(intents.some((intent) => intent.type === "look")).toBe(true);
  });

  it("does not emit look during a short idle period", () => {
    const intents = decide({ event: { type: "idle-tick", elapsedMs: 2_000 }, state: baseState, policy: emptyPolicy });
    expect(intents.some((intent) => intent.type === "look")).toBe(false);
  });

  it("suppresses a repeat interaction within cooldown", () => {
    const policy = { cooldowns: { "react-happy": Number.MAX_SAFE_INTEGER } };
    const intents = decide({
      event: { type: "double-clicked" },
      state: baseState,
      policy,
    });
    expect(intents).not.toContainEqual({ type: "react-happy" });
  });

  it("allows an interaction after its cooldown expired", () => {
    const policy = { cooldowns: { "react-happy": 1 } };
    const intents = decide({
      event: { type: "double-clicked" },
      state: baseState,
      policy,
    });
    expect(intents).toContainEqual({ type: "react-happy" });
  });
});
