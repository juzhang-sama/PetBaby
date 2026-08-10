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
});
