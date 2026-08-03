import { describe, expect, it } from "vitest";
import { evolveState } from "./state";
import type { PetStateSnapshot } from "./state";

const base: PetStateSnapshot = {
  schemaVersion: 1,
  petId: "pet-1",
  energy: 0.7,
  mood: 0.6,
  bond: 0.3,
  lastSeenAt: "2026-08-03T00:00:00.000Z",
  lastInteractionAt: "2026-08-03T00:00:00.000Z",
};

describe("evolveState", () => {
  it("drains energy slowly over idle time", () => {
    const now = new Date("2026-08-03T01:00:00.000Z"); // 1 hour later
    const next = evolveState(base, now, 3_600_000);
    expect(next.energy).toBeLessThan(base.energy);
    expect(next.energy).toBeGreaterThan(0);
  });

  it("does not drop energy below zero", () => {
    const now = new Date("2026-08-03T10:00:00.000Z"); // 10 hours later
    const next = evolveState(base, now, 36_000_000);
    expect(next.energy).toBeGreaterThanOrEqual(0);
  });

  it("raises mood on interaction but caps at 1", () => {
    const next = evolveState(base, new Date("2026-08-03T00:00:10.000Z"), 0, true);
    expect(next.mood).toBeGreaterThan(base.mood);
    const saturated = evolveState({ ...base, mood: 0.99 }, new Date("2026-08-03T00:00:20.000Z"), 0, true);
    expect(saturated.mood).toBeLessThanOrEqual(1);
  });

  it("raises bond slowly on interaction", () => {
    const next = evolveState(base, new Date("2026-08-03T00:00:10.000Z"), 0, true);
    expect(next.bond).toBeGreaterThan(base.bond);
  });

  it("never punishes absence: energy floor is above zero for realistic gaps", () => {
    const now = new Date("2026-08-03T03:00:00.000Z"); // 3 hours away
    const next = evolveState(base, now, 10_800_000);
    expect(next.energy).toBeGreaterThan(0);
  });

  it("updates lastSeenAt to the current time", () => {
    const now = new Date("2026-08-03T01:00:00.000Z");
    const next = evolveState(base, now, 3_600_000);
    expect(next.lastSeenAt).toBe(now.toISOString());
  });
});
