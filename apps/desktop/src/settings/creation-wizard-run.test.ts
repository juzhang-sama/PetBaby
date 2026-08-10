import { describe, expect, it } from "vitest";
import { CreationWizardRun } from "./creation-wizard-run";

describe("CreationWizardRun", () => {
  it("allows one submission for the current wizard visit", () => {
    const run = new CreationWizardRun();
    const visit = run.enter();
    expect(run.beginSubmission(visit)).toBe(true);
    expect(run.beginSubmission(visit)).toBe(false);
    run.endSubmission(visit);
    expect(run.beginSubmission(visit)).toBe(true);
  });

  it("compensates a pet created by an old visit without taking over the new visit", () => {
    const run = new CreationWizardRun();
    const oldVisit = run.enter();
    const newVisit = run.enter();
    expect(run.shouldCompensateCreatedPet(oldVisit)).toBe(true);
    expect(run.shouldPersistPet(oldVisit)).toBe(false);
    expect(run.shouldCompensateCreatedPet(newVisit)).toBe(false);
    expect(run.shouldPersistPet(newVisit)).toBe(true);
  });

  it("locks one pet's compile across a re-entered visit without blocking another pet", () => {
    const run = new CreationWizardRun();
    const first = run.enter();
    expect(run.beginOperation(first, "compile", "pet-1")).not.toBeNull();
    expect(run.beginOperation(first, "compile", "pet-1")).toBeNull();
    const reentered = run.enter();
    expect(run.beginOperation(reentered, "compile", "pet-1")).toBeNull();
    expect(run.beginOperation(reentered, "compile", "pet-2")).not.toBeNull();
    expect(run.isPetBusy("pet-1")).toBe(true);
    expect(run.isPetBusy("pet-2")).toBe(true);
  });

  it("locks activation per pet without globally blocking a different pet", () => {
    const run = new CreationWizardRun();
    const first = run.enter();
    const firstActivation = run.beginOperation(first, "activate", "pet-1");
    expect(firstActivation).not.toBeNull();
    const reentered = run.enter();
    expect(run.beginOperation(reentered, "activate", "pet-1")).toBeNull();
    expect(run.beginOperation(reentered, "activate", "pet-2")).not.toBeNull();
    run.settleOperation(firstActivation!);
    expect(run.beginOperation(reentered, "activate", "pet-1")).not.toBeNull();
  });

  it.each(["compile", "activate"] as const)("refreshes stale %s only when pet revision still matches", (kind) => {
    const run = new CreationWizardRun();
    const oldVisit = run.enter();
    expect(run.beginGeneration(oldVisit, "pet-1")).toBe(1);
    const operation = run.beginOperation(oldVisit, kind, "pet-1");
    const currentVisit = run.enter();
    expect(run.shouldRefreshStaleOperation(operation!, "pet-1")).toBe(true);
    const refresh = run.beginRefresh(currentVisit, "pet-1", operation!.revision);
    expect(refresh).not.toBeNull();
    expect(run.beginGeneration(currentVisit, "pet-1")).toBe(2);
    expect(run.shouldRefreshStaleOperation(operation!, "pet-1")).toBe(false);
    expect(run.shouldApplyRefresh(refresh!, "pet-1")).toBe(false);
  });

  it.each([
    ["compile", "success"],
    ["compile", "failure"],
    ["activate", "success"],
    ["activate", "failure"],
  ] as const)("settling stale %s %s releases its pet lock before refresh", (kind, _outcome) => {
    const run = new CreationWizardRun();
    const oldVisit = run.enter();
    const operation = run.beginOperation(oldVisit, kind, "pet-1");
    run.enter();
    expect(run.isPetBusy("pet-1")).toBe(true);
    run.settleOperation(operation!);
    expect(run.isPetBusy("pet-1")).toBe(false);
    expect(run.shouldRefreshStaleOperation(operation!, "pet-1")).toBe(true);
  });

  it("never refreshes a different current pet after a stale operation settles", () => {
    const run = new CreationWizardRun();
    const oldVisit = run.enter();
    const operation = run.beginOperation(oldVisit, "compile", "pet-1");
    run.enter();
    expect(run.shouldRefreshStaleOperation(operation!, "pet-2")).toBe(false);
  });
});
