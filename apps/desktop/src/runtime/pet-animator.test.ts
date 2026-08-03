import { describe, expect, it, vi } from "vitest";
import { BlinkScheduler, breathPhaseAt, PetAnimator } from "./pet-animator";
import type { AnimatorDriver } from "./pet-animator";

function fakeDriver(): AnimatorDriver & { calls: string[] } {
  return {
    calls: [],
    setEyesOpen(open) { this.calls.push(`eyes:${open}`); },
    setBreathPhase() { this.calls.push("breath"); },
    scaleSquash() { this.calls.push("squash"); },
    shift() { this.calls.push("shift"); },
    setAccentVisible() { this.calls.push("accent"); },
  };
}

describe("breathPhaseAt", () => {
  it("cycles with a 4 second period", () => {
    expect(breathPhaseAt(0)).toBeCloseTo(0);
    expect(breathPhaseAt(1_000)).toBeCloseTo(0.25, 3);
    expect(breathPhaseAt(4_000)).toBeCloseTo(0, 3);
  });
});

describe("BlinkScheduler", () => {
  it("schedules a blink inside the configured window", () => {
    const now = 1_000_000;
    const scheduler = new BlinkScheduler(3_000, 8_000, now);
    const next = scheduler.nextAt(now);
    expect(next).toBeGreaterThanOrEqual(now + 3_000);
    expect(next).toBeLessThanOrEqual(now + 8_000);
  });

  it("advances the window after a blink", () => {
    const now = 1_000_000;
    const scheduler = new BlinkScheduler(3_000, 8_000, now);
    const first = scheduler.nextAt(now);
    const second = scheduler.nextAt(first + 1);
    expect(second).toBeGreaterThan(first);
  });
});

describe("PetAnimator", () => {
  it("drives breathing while idle", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.tick(0);
    animator.tick(500);
    animator.stop();
    expect(driver.calls.filter((call) => call === "breath").length).toBeGreaterThanOrEqual(1);
  });

  it("closes eyes during a blink and reopens them", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.tick(0);
    // force a blink
    animator.forceBlink();
    animator.tick(50);
    expect(driver.calls).toContain("eyes:false");
    animator.tick(300);
    expect(driver.calls).toContain("eyes:true");
    animator.stop();
  });

  it("switches to carried mode and stops breathing motion", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    const breathCallsBefore = driver.calls.filter((c) => c === "breath").length;
    animator.setMode("carried");
    animator.tick(1_000);
    const breathCallsAfter = driver.calls.filter((c) => c === "breath").length;
    expect(breathCallsAfter).toBe(breathCallsBefore);
    animator.stop();
  });

  it("plays a bounce for react-happy intent", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.setIntent({ type: "react-happy" });
    animator.tick(0);
    animator.tick(80);
    expect(driver.calls).toContain("squash");
    animator.stop();
  });
});
