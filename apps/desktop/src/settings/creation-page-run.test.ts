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

  it("serializes every mutation kind for the same session", () => {
    const run = new CreationPageRun();
    const visit = run.enter("upload");
    const finalize = run.begin(visit, "finalize", "session-1");

    expect(finalize).not.toBeNull();
    expect(run.begin(visit, "retry", "session-1")).toBeNull();
    expect(run.begin(visit, "abandon", "session-1")).toBeNull();
    expect(run.begin(visit, "submit", "session-1")).toBeNull();
    expect(run.isMutating("session-1")).toBe(true);

    run.settle(finalize!);
    expect(run.isMutating("session-1")).toBe(false);
    expect(run.begin(visit, "abandon", "session-1")).not.toBeNull();
  });

  it("does not let settling an old visit unlock the current visit mutation", () => {
    const run = new CreationPageRun();
    const firstVisit = run.enter("upload");
    const old = run.begin(firstVisit, "finalize", "session-1")!;
    run.leave();
    const currentVisit = run.enter("upload");
    const current = run.begin(currentVisit, "retry", "session-1")!;

    run.settle(old);

    expect(run.isMutating("session-1")).toBe(true);
    expect(run.begin(currentVisit, "abandon", "session-1")).toBeNull();
    run.settle(current);
    expect(run.isMutating("session-1")).toBe(false);
  });

  it("does not let an old poll settlement unlock the current visit poll", () => {
    const run = new CreationPageRun();
    const firstVisit = run.enter("upload");
    const old = run.begin(firstVisit, "poll", "session-1")!;
    run.leave();
    const currentVisit = run.enter("upload");
    const current = run.begin(currentVisit, "poll", "session-1")!;

    run.settle(old);

    expect(run.begin(currentVisit, "poll", "session-1")).toBeNull();
    run.settle(current);
    expect(run.begin(currentVisit, "poll", "session-1")).not.toBeNull();
  });
});
