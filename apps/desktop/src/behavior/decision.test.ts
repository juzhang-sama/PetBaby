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

  it("maps a thrown drag-end to falling", () => {
    const intents = decide({
      event: { type: "drag-end", velocity: { x: 400, y: -300 } },
      state: baseState,
      policy: emptyPolicy,
    });
    expect(intents).toContainEqual({ type: "falling" });
  });

  it("maps a landed event to the landed intent", () => {
    const intents = decide({ event: { type: "landed" }, state: baseState, policy: emptyPolicy });
    expect(intents).toContainEqual({ type: "landed" });
  });

  it("maps petted to a curious reaction", () => {
    const intents = decide({ event: { type: "petted" }, state: baseState, policy: emptyPolicy });
    expect(intents).toContainEqual({ type: "react-curious" });
  });

  it("maps fed and played to happy reactions", () => {
    const fed = decide({ event: { type: "fed" }, state: baseState, policy: emptyPolicy });
    const played = decide({ event: { type: "played" }, state: baseState, policy: emptyPolicy });
    expect(fed).toContainEqual({ type: "react-happy" });
    expect(played).toContainEqual({ type: "react-happy" });
  });

  it("suppresses interactions within their own cooldowns", () => {
    const policy = {
      cooldowns: {
        pet: Number.MAX_SAFE_INTEGER,
        feed: Number.MAX_SAFE_INTEGER,
        play: Number.MAX_SAFE_INTEGER,
      },
    };
    expect(decide({ event: { type: "petted" }, state: baseState, policy })).not.toContainEqual({ type: "react-curious" });
    expect(decide({ event: { type: "fed" }, state: baseState, policy })).not.toContainEqual({ type: "react-happy" });
    expect(decide({ event: { type: "played" }, state: baseState, policy })).not.toContainEqual({ type: "react-happy" });
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
