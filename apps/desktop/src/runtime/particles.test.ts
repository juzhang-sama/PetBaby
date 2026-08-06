import { describe, expect, it } from "vitest";
import { ParticleSystem } from "./particles";

describe("ParticleSystem", () => {
  it("spawns particles and removes them after their lifetime", () => {
    const system = new ParticleSystem();
    system.spawn("heart", 100, 100, { count: 3 });
    expect(system.count).toBe(3);
    system.update(1_300);
    expect(system.count).toBe(0);
  });

  it("moves particles upward and removes them gradually", () => {
    const system = new ParticleSystem();
    system.spawn("heart", 100, 100);
    const before = { y: system.active[0]!.y, life: system.active[0]!.life };
    system.update(100);
    const after = system.active[0]!;
    expect(after.y).toBeLessThan(before.y);
    expect(after.life).toBeLessThan(before.life);
  });

  it("keeps zzz particles longer than sparks", () => {
    const system = new ParticleSystem();
    system.spawn("zzz", 0, 0);
    system.spawn("spark", 0, 0);
    system.update(900);
    const kinds = system.active.map((p) => p.kind);
    expect(kinds).toContain("zzz");
    expect(kinds).not.toContain("spark");
  });

  it("clears all particles", () => {
    const system = new ParticleSystem();
    system.spawn("heart", 0, 0);
    system.clear();
    expect(system.count).toBe(0);
  });
});
