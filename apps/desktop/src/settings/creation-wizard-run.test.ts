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

  it("invalidates work when leaving or replacing the wizard visit", () => {
    const run = new CreationWizardRun();
    const first = run.enter();
    expect(run.isCurrent(first)).toBe(true);
    const second = run.enter();
    expect(run.isCurrent(first)).toBe(false);
    expect(run.isCurrent(second)).toBe(true);
    run.leave();
    expect(run.isCurrent(second)).toBe(false);
  });

  it("compensates a pet created by an old visit without taking over the new visit", () => {
    const run = new CreationWizardRun();
    const oldVisit = run.enter();
    const newVisit = run.enter();
    expect(run.shouldCompensateCreatedPet(oldVisit)).toBe(true);
    expect(run.shouldCompensateCreatedPet(newVisit)).toBe(false);
    expect(run.shouldPersistPet(oldVisit)).toBe(false);
    expect(run.shouldPersistPet(newVisit)).toBe(true);
  });

  it("does not globally block activation after a visit change", () => {
    const run = new CreationWizardRun();
    const oldVisit = run.enter();
    expect(run.beginActivation(oldVisit, "pet-1")).toBe(true);
    const newVisit = run.enter();
    expect(run.beginActivation(newVisit, "pet-1")).toBe(false);
    expect(run.beginActivation(newVisit, "pet-2")).toBe(true);
    run.endActivation("pet-1");
    expect(run.beginActivation(newVisit, "pet-1")).toBe(true);
  });

  it.each([
    ["compile", "success"],
    ["compile", "failure"],
    ["activate", "success"],
    ["activate", "failure"],
  ] as const)("refreshes the current same-pet visit after stale %s %s", (operation, outcome) => {
    const run = new CreationWizardRun();
    const oldVisit = run.enter();
    run.enter();
    expect(run.settledStaleOperation(oldVisit, operation, outcome, "pet-1", "pet-1")).toEqual({
      refreshCurrentPet: true,
      resetCurrentControls: true,
    });
    expect(run.settledStaleOperation(oldVisit, operation, outcome, "pet-1", "pet-2")).toEqual({
      refreshCurrentPet: false,
      resetCurrentControls: false,
    });
  });
});
