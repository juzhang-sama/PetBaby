import { describe, expect, it } from "vitest";
import { CreationPageRun } from "./creation-page-run";

describe("CreationPageRun", () => {
  it("ignores a previous visit candidate after leaving upload", () => {
    const run = new CreationPageRun();
    const old = run.enter("upload");
    run.leave();
    const current = run.enter("composer");

    expect(run.isCurrent(old)).toBe(false);
    expect(run.isCurrent(current)).toBe(true);
  });

  it("does not apply a poll that settles after a newer visit", () => {
    const run = new CreationPageRun();
    const oldVisit = run.enter("upload");
    const poll = run.begin(oldVisit, "poll", "session-1");
    run.leave();
    run.enter("upload");

    expect(run.shouldApply(poll!, "session-1")).toBe(false);
  });

  it("allows only one finalization for a session until it settles", () => {
    const run = new CreationPageRun();
    const visit = run.enter("upload");
    const first = run.begin(visit, "finalize", "session-1");

    expect(first).not.toBeNull();
    expect(run.begin(visit, "finalize", "session-1")).toBeNull();
    run.settle(first!);
    expect(run.begin(visit, "finalize", "session-1")).not.toBeNull();
  });
});
