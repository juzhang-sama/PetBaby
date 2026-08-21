import { describe, expect, it, vi } from "vitest";
import type { CreationSnapshot } from "../creation/contracts";
import {
  CreationPageActivity,
  CreationPageFocusManager,
  CreationPageRun,
} from "./creation-page-run";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => { resolve = onResolve; reject = onReject; });
  return { promise, resolve, reject };
}

function snapshot(method: "upload" | "composer", sessionId = `session-${method}`): CreationSnapshot {
  return {
    sessionId,
    petId: `pet-${method}`,
    method,
    status: "draft",
    lastStableStatus: "draft",
    currentStep: method === "composer" ? "ears" : "upload",
    displayName: null,
    jobId: null,
    jobStatus: null,
    candidateId: null,
    recipe: null,
    error: null,
  };
}

function routerPorts(options: {
  draft?: CreationSnapshot | null;
  draftError?: Error;
  choice?: "continue" | "abandon" | "cancel";
} = {}) {
  const creation = {
    draft: options.draftError
      ? vi.fn(async () => { throw options.draftError; })
      : vi.fn(async () => options.draft ?? null),
    abandon: vi.fn(async () => undefined),
  };
  const views = {
    upload: { open: vi.fn(async (_sessionId: string | null) => undefined), leave: vi.fn() },
    composer: { open: vi.fn(async (_sessionId: string | null) => undefined), leave: vi.fn() },
    adoption: { open: vi.fn(async (_sessionId: string | null) => undefined), leave: vi.fn() },
  };
  const dialog = {
    showDraftChoice: vi.fn(async (_method: "upload" | "composer") => options.choice ?? "cancel" as const),
  };
  const onRoute = vi.fn();
  const onBusy = vi.fn();
  return {
    creation, views, dialog, onRoute, onBusy,
    page: new CreationPageRun({ creation, views, dialog, onRoute, onBusy }),
  };
}

describe("CreationPageRun", () => {
  it("serializes mutations from every creation route under one shared busy owner", async () => {
    const first = deferred<void>();
    const order: string[] = [];
    const onBusy = vi.fn();
    const activity = new CreationPageActivity(onBusy);

    const upload = activity.run(
      { route: "upload", kind: "finalize", sessionId: "session-upload" },
      async () => { order.push("upload-start"); await first.promise; order.push("upload-end"); },
    );
    const composer = activity.run(
      { route: "composer", kind: "save", sessionId: "session-composer" },
      async () => { order.push("composer"); },
    );
    await Promise.resolve();

    expect(order).toEqual(["upload-start"]);
    expect(activity.isBusy()).toBe(true);
    first.resolve();
    await Promise.all([upload, composer]);

    expect(order).toEqual(["upload-start", "upload-end", "composer"]);
    expect(onBusy.mock.calls.map(([busy]) => busy)).toEqual([true, false]);
    expect(activity.isBusy()).toBe(false);
  });

  it("moves focus into a route and returns it to the route trigger", () => {
    const trigger = { focus: vi.fn() } as unknown as HTMLElement;
    const firstControl = { focus: vi.fn() } as unknown as HTMLElement;
    const workspace = {
      querySelector: vi.fn(() => firstControl),
    } as unknown as HTMLElement;
    const focus = new CreationPageFocusManager((callback) => callback());

    focus.remember("adoption", trigger);
    focus.enter("adoption", workspace);
    focus.returnToTrigger("adoption");

    expect(firstControl.focus).toHaveBeenCalledOnce();
    expect(trigger.focus).toHaveBeenCalledOnce();
  });

  it("cancels a scheduled route focus when the settings tab leaves creation", () => {
    const scheduled: Array<() => void> = [];
    const firstControl = { focus: vi.fn() } as unknown as HTMLElement;
    const workspace = { querySelector: () => firstControl } as unknown as HTMLElement;
    const focus = new CreationPageFocusManager((callback) => { scheduled.push(callback); });

    focus.enter("upload", workspace);
    focus.cancel();
    scheduled[0]!();

    expect(firstControl.focus).not.toHaveBeenCalled();
  });

  it("offers exactly upload composer and adoption entries", () => {
    expect(new CreationPageRun().routes()).toEqual(["upload", "composer", "adoption"]);
  });

  it("asks to continue or abandon before replacing a long draft", async () => {
    const test = routerPorts({ draft: snapshot("composer"), choice: "cancel" });

    await test.page.open("upload");

    expect(test.dialog.showDraftChoice).toHaveBeenCalledWith("composer");
    expect(test.views.upload.open).not.toHaveBeenCalled();
    expect(test.creation.abandon).not.toHaveBeenCalled();
  });

  it("continues the durable current draft instead of opening the requested method", async () => {
    const draft = snapshot("composer");
    const test = routerPorts({ draft, choice: "continue" });

    await test.page.open("upload");

    expect(test.views.composer.open).toHaveBeenCalledWith(draft.sessionId);
    expect(test.views.upload.open).not.toHaveBeenCalled();
    expect(test.onRoute).toHaveBeenLastCalledWith("composer");
  });

  it("abandons an upload draft and starts fresh through the upload route", async () => {
    const draft = snapshot("upload", "photo-avatar-session");
    const test = routerPorts({ draft });

    await test.page.open("upload");

    expect(test.creation.abandon).toHaveBeenCalledWith("photo-avatar-session");
    expect(test.views.upload.open).toHaveBeenCalledWith(null);
    expect(test.dialog.showDraftChoice).not.toHaveBeenCalled();
    expect(test.onRoute).toHaveBeenLastCalledWith("upload");
  });

  it("abandons before opening a different long-lived method", async () => {
    const draft = snapshot("composer");
    const test = routerPorts({ draft, choice: "abandon" });

    await test.page.open("upload");

    expect(test.creation.abandon).toHaveBeenCalledWith(draft.sessionId);
    expect(test.views.upload.open).toHaveBeenCalledWith(null);
  });

  it("does not treat a draft read failure as no draft", async () => {
    const test = routerPorts({ draftError: new Error("database unavailable") });

    await expect(test.page.open("upload")).rejects.toThrow(/database unavailable/);

    expect(test.views.upload.open).not.toHaveBeenCalled();
    expect(test.creation.abandon).not.toHaveBeenCalled();
  });

  it("opening adoption does not read or abandon a long-lived draft", async () => {
    const test = routerPorts({ draft: snapshot("composer") });

    await test.page.open("adoption");

    expect(test.creation.draft).not.toHaveBeenCalled();
    expect(test.creation.abandon).not.toHaveBeenCalled();
    expect(test.views.adoption.open).toHaveBeenCalledWith(null);
  });

  it("keeps one global busy interval and prevents an old route visit from overwriting the latest", async () => {
    let resolveDraft!: (value: CreationSnapshot | null) => void;
    const draftPromise = new Promise<CreationSnapshot | null>((resolve) => { resolveDraft = resolve; });
    const test = routerPorts();
    test.creation.draft.mockImplementationOnce(async () => draftPromise);

    const oldUpload = test.page.open("upload");
    const currentAdoption = test.page.open("adoption");
    await currentAdoption;
    resolveDraft(null);
    await oldUpload;

    expect(test.views.upload.open).not.toHaveBeenCalled();
    expect(test.views.adoption.open).toHaveBeenCalledTimes(1);
    expect(test.onRoute).toHaveBeenCalledTimes(1);
    expect(test.onRoute).toHaveBeenLastCalledWith("adoption");
    expect(test.onBusy.mock.calls.map(([busy]) => busy)).toEqual([true, false]);
  });

  it("leaves the active route exactly once when the page closes repeatedly", async () => {
    const test = routerPorts();
    await test.page.open("adoption");

    test.page.close();
    test.page.close();

    expect(test.views.adoption.leave).toHaveBeenCalledTimes(1);
  });

  it("waits for the shared mutation settlement before changing routes", async () => {
    const idle = deferred<void>();
    const test = routerPorts();
    const page = new CreationPageRun({
      creation: test.creation,
      views: test.views,
      dialog: test.dialog,
      onRoute: test.onRoute,
      onBusy: test.onBusy,
      activity: {
        waitForIdle: () => idle.promise,
        run: async (_owner, operation) => operation(),
      },
    });

    const opening = page.open("adoption");
    await Promise.resolve();
    expect(test.views.adoption.open).not.toHaveBeenCalled();

    idle.resolve();
    await opening;
    expect(test.views.adoption.open).toHaveBeenCalledOnce();
  });

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
    expect(run.begin(currentVisit, "retry", "session-1")).toBeNull();

    run.settle(old);

    const current = run.begin(currentVisit, "retry", "session-1")!;
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

  it("lets a new visit await the exact cross-visit mutation owner", async () => {
    const run = new CreationPageRun();
    const oldVisit = run.enter("upload");
    const owner = run.begin(oldVisit, "finalize", "session-1")!;
    run.leave();
    run.enter("upload");
    let settled = false;

    const waiting = run.waitForMutation("session-1").then(() => { settled = true; });
    await Promise.resolve();
    expect(settled).toBe(false);

    run.settle(owner);
    await waiting;
    expect(settled).toBe(true);
  });

  it("reports every active mutation owner and shares one settlement promise per session", () => {
    const run = new CreationPageRun();
    const visit = run.enter("upload");
    run.begin(visit, "retry", "session-1");
    run.begin(visit, "finalize", "session-2");

    expect(run.activeMutations()).toEqual([
      { kind: "retry", sessionId: "session-1" },
      { kind: "finalize", sessionId: "session-2" },
    ]);
    expect(run.waitForMutation("session-1")).toBe(run.waitForMutation("session-1"));
  });
});
