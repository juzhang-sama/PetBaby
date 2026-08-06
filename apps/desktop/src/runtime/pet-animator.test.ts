import { describe, expect, it, vi } from "vitest";
import { BlinkScheduler, breathPhaseAt, PetAnimator } from "./pet-animator";
import type { AnimatorDriver } from "./pet-animator";

function fakeDriver(): AnimatorDriver & {
  calls: string[];
  tilts: number[];
  headTurns: number[];
} {
  return {
    calls: [],
    tilts: [],
    headTurns: [],
    setEyesOpen(open) { this.calls.push(`eyes:${open}`); },
    setBreathPhase() { this.calls.push("breath"); },
    scaleSquash() { this.calls.push("squash"); },
    shift() { this.calls.push("shift"); },
    setAccentVisible() { this.calls.push("accent"); },
    setTilt(angle) { this.tilts.push(angle); this.calls.push("tilt"); },
    setHeadTurn(amount) { this.headTurns.push(amount); this.calls.push("head-turn"); },
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

  it("automatically blinks on schedule", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.tick(0); // schedules the first blink inside [3000, 8000]
    animator.tick(9_000); // now past any scheduled time: blink fires
    animator.tick(9_050); // within the 160ms closed window
    expect(driver.calls).toContain("eyes:false");
    animator.tick(9_300); // window passed: eyes reopen
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

  it("resumes breathing after landing", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.setIntent({ type: "carried" });
    animator.tick(1_000);
    const breathCallsCarried = driver.calls.filter((c) => c === "breath").length;
    animator.setIntent({ type: "landed" });
    animator.tick(2_000);
    animator.tick(2_500);
    const breathCallsAfter = driver.calls.filter((c) => c === "breath").length;
    expect(breathCallsAfter).toBeGreaterThan(breathCallsCarried);
    animator.stop();
  });

  it("resets the bounce offset after the bounce finishes", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.setIntent({ type: "react-happy" });
    animator.tick(0);
    animator.tick(400);
    expect(driver.calls).toContain("shift");
    const shifts = driver.calls.filter((c) => c === "shift").length;
    animator.tick(700);
    expect(driver.calls.filter((c) => c === "shift").length).toBe(shifts + 1);
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

  it("tilts toward the look target and returns to center", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.tick(0);
    animator.setIntent({ type: "look", target: "left" });
    animator.tick(200);
    animator.tick(600);
    const tilted = Math.max(...driver.tilts);
    expect(tilted).toBeGreaterThan(2);
    animator.tick(1_200);
    animator.tick(2_000);
    expect(driver.tilts.at(-1)).toBeCloseTo(0, 1);
    animator.stop();
  });

  it("turns the head toward the look target and centers it afterwards", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.tick(0);
    animator.setIntent({ type: "look", target: "left" });
    animator.tick(200);
    const turned = Math.max(...driver.headTurns);
    expect(turned).toBeGreaterThan(0.5);
    animator.tick(2_000);
    expect(driver.headTurns.at(-1)).toBeCloseTo(0, 1);
    animator.stop();
  });

  it("keeps eyes closed while sleeping", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.setIntent({ type: "sleep" });
    animator.tick(0);
    animator.tick(500);
    expect(driver.calls.filter((call) => call === "eyes:false").length).toBeGreaterThan(0);
    expect(driver.calls).not.toContain("eyes:true");
    animator.stop();
  });

  it("wobbles while carried", () => {
    const driver = fakeDriver();
    const animator = new PetAnimator(driver);
    animator.start();
    animator.setIntent({ type: "carried" });
    for (let i = 0; i < 20; i += 1) {
      animator.tick(1_000 + i * 80);
    }
    const wobble = driver.tilts.filter((angle) => angle !== 0);
    expect(wobble.length).toBeGreaterThan(10);
    animator.stop();
  });
});
