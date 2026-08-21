import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_PET_CALIBRATION,
  type PetCalibrationV1,
} from "../runtime/pet-calibration";
import {
  PetCalibrationCatalogCoordinator,
  PetCalibrationControl,
  type PetCalibrationControlElements,
  type PetCalibrationControlPorts,
} from "./pet-calibration-control";

type Listener = (event: { currentTarget?: FakeElement }) => void;

class FakeElement {
  value = "";
  textContent = "";
  disabled = false;
  hidden = false;
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>();
  private listeners = new Map<string, Set<Listener>>();

  addEventListener(type: string, listener: Listener): void {
    const listeners = this.listeners.get(type) ?? new Set<Listener>();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  dispatch(type: string): void {
    for (const listener of this.listeners.get(type) ?? []) listener({ currentTarget: this });
  }

  listenerCount(): number {
    return [...this.listeners.values()].reduce((total, listeners) => total + listeners.size, 0);
  }
}

class FakeClock {
  private nextId = 1;
  private callbacks = new Map<number, () => void>();
  readonly delays: number[] = [];

  setTimeout = (callback: () => void, delayMs: number): number => {
    const id = this.nextId++;
    this.delays.push(delayMs);
    this.callbacks.set(id, callback);
    return id;
  };

  clearTimeout = (id: number): void => {
    this.callbacks.delete(id);
  };

  flush(): void {
    const pending = [...this.callbacks.values()];
    this.callbacks.clear();
    for (const callback of pending) callback();
  }

  get pendingCount(): number { return this.callbacks.size; }
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason: unknown): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const calibration = (overrides: Partial<PetCalibrationV1> = {}): PetCalibrationV1 => ({
  ...DEFAULT_PET_CALIBRATION,
  ...overrides,
});

function mount(options: {
  saved?: PetCalibrationV1;
  load?: PetCalibrationControlPorts["load"];
  save?: PetCalibrationControlPorts["save"];
  runtime?: PetCalibrationControlPorts["runtime"];
} = {}) {
  const elements = {
    root: new FakeElement(),
    petName: new FakeElement(),
    breath: new FakeElement(),
    breathOutput: new FakeElement(),
    feedback: new FakeElement(),
    feedbackOutput: new FakeElement(),
    reset: new FakeElement(),
    feedbackTest: new FakeElement(),
    cancel: new FakeElement(),
    save: new FakeElement(),
    status: new FakeElement(),
    error: new FakeElement(),
  } as unknown as PetCalibrationControlElements;
  const clock = new FakeClock();
  const savedValues: PetCalibrationV1[] = [];
  const runtimeActions: Array<{ petId: string; action: string; value: PetCalibrationV1 }> = [];
  const initial = options.saved ?? calibration();
  const ports: PetCalibrationControlPorts = {
    load: options.load ?? (async () => initial),
    save: options.save ?? (async (_petId, value) => {
      savedValues.push(value);
      return value;
    }),
    runtime: options.runtime ?? (async (petId, action, value) => {
      runtimeActions.push({ petId, action, value });
    }),
  };
  const control = new PetCalibrationControl({ elements, ports, clock });
  control.mount();
  return {
    control,
    elements: elements as unknown as Record<keyof PetCalibrationControlElements, FakeElement>,
    clock,
    savedValues,
    runtimeActions,
    async settle() {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    },
  };
}

describe("PetCalibrationControl", () => {
  it("requires close restore only after a runtime preview and clears it after restore", async () => {
    const view = mount();
    await view.control.open("pet-a", "小白");
    expect(view.control.needsRestoreBeforeClose()).toBe(false);

    await view.control.reset();
    expect(view.control.needsRestoreBeforeClose()).toBe(true);

    await view.control.cancel();
    expect(view.control.needsRestoreBeforeClose()).toBe(false);
  });

  it("keeps a failed load unavailable and retries the same pet until canonical data loads", async () => {
    const load = vi.fn()
      .mockRejectedValueOnce(new Error("corrupt calibration"))
      .mockResolvedValueOnce(calibration({ breathAmplitudePercent: 3 }));
    const view = mount({ load });

    expect(await view.control.open("pet-a", "小白")).toBe(false);
    expect(view.elements.breath.disabled).toBe(true);
    expect(view.elements.save.disabled).toBe(true);
    expect(view.elements.error.textContent).toContain("corrupt calibration");
    view.elements.breath.value = "5";
    view.elements.breath.dispatch("input");
    await view.control.save();
    expect(view.savedValues).toEqual([]);

    expect(await view.control.open("pet-a", "小白")).toBe(true);
    expect(load).toHaveBeenCalledTimes(2);
    expect(view.elements.breath.disabled).toBe(false);
    expect(view.elements.save.disabled).toBe(false);
    expect(view.elements.breath.value).toBe("3");
  });

  it("reset previews defaults but persists only after save", async () => {
    const view = mount({ saved: calibration({ breathAmplitudePercent: 4 }) });
    await view.control.open("pet-a", "小白");

    await view.control.reset();
    expect(view.runtimeActions.at(-1)).toEqual({
      petId: "pet-a",
      action: "preview",
      value: DEFAULT_PET_CALIBRATION,
    });
    expect(view.savedValues).toHaveLength(0);

    await view.control.save();
    expect(view.savedValues).toEqual([DEFAULT_PET_CALIBRATION]);
    expect(view.runtimeActions.at(-1)).toEqual({
      petId: "pet-a",
      action: "commit",
      value: DEFAULT_PET_CALIBRATION,
    });
  });

  it("debounces slider previews for 100ms and exposes semantic values", async () => {
    const view = mount();
    await view.control.open("pet-a", "小白");
    view.elements.breath.value = "4.2";
    view.elements.breath.dispatch("input");
    view.elements.feedback.value = "0.75";
    view.elements.feedback.dispatch("input");

    expect(view.clock.delays).toEqual([100, 100]);
    expect(view.clock.pendingCount).toBe(1);
    expect(view.runtimeActions).toHaveLength(0);
    expect(view.elements.breathOutput.textContent).toBe("4.2%");
    expect(view.elements.feedbackOutput.textContent).toBe("75%");
    expect(view.elements.breath.attributes.get("aria-valuetext")).toBe("呼吸幅度 4.2%");

    view.clock.flush();
    await view.settle();
    expect(view.runtimeActions).toEqual([{
      petId: "pet-a",
      action: "preview",
      value: calibration({ breathAmplitudePercent: 4.2, feedbackStrength: 0.75 }),
    }]);
  });

  it("tests click feedback with the current draft without persisting", async () => {
    const view = mount();
    await view.control.open("pet-a", "小白");
    view.elements.feedback.value = "0.25";
    view.elements.feedback.dispatch("input");

    await view.control.previewFeedback();

    expect(view.savedValues).toEqual([]);
    expect(view.runtimeActions.at(-1)).toEqual({
      petId: "pet-a",
      action: "feedback",
      value: calibration({ feedbackStrength: 0.25 }),
    });
  });

  it("ignores a stale feedback failure after newer slider input", async () => {
    const feedback = deferred<void>();
    const actions: string[] = [];
    const view = mount({
      saved: calibration({ feedbackStrength: 0.4 }),
      runtime: async (_petId, action) => {
        actions.push(action);
        if (action === "feedback") return feedback.promise;
      },
    });
    await view.control.open("pet-a", "小白");
    const pendingFeedback = view.control.previewFeedback();
    view.elements.feedback.value = "0.85";
    view.elements.feedback.dispatch("input");

    feedback.reject(new Error("late feedback failure"));
    await pendingFeedback;

    expect(view.elements.feedback.value).toBe("0.85");
    expect(view.elements.error.textContent).not.toContain("late feedback failure");
    expect(actions).toEqual(["feedback"]);
    expect(view.clock.pendingCount).toBe(1);
  });

  it.each(["preview", "feedback"] as const)(
    "restores the saved snapshot when a %s runtime request fails",
    async (failingAction) => {
      const saved = calibration({ feedbackStrength: 0.45 });
      const actions: string[] = [];
      const view = mount({
        saved,
        runtime: async (_petId, action) => {
          actions.push(action);
          if (action === failingAction) throw new Error("pet unavailable");
        },
      });
      await view.control.open("pet-a", "小白");

      if (failingAction === "preview") {
        view.elements.feedback.value = "1";
        view.elements.feedback.dispatch("input");
        view.clock.flush();
        await view.settle();
      } else {
        await view.control.previewFeedback();
      }

      expect(actions).toEqual([failingAction, "restore"]);
      expect(view.elements.feedback.value).toBe("0.45");
      expect(view.elements.error.textContent).toContain("pet unavailable");
    },
  );

  it("commits only the backend canonical value after save succeeds", async () => {
    const order: string[] = [];
    const canonical = calibration({ breathAmplitudePercent: 3.25 });
    const view = mount({
      save: async () => {
        order.push("backend-save");
        return canonical;
      },
      runtime: async (_petId, action, value) => {
        order.push(`runtime-${action}-${value.breathAmplitudePercent}`);
      },
    });
    await view.control.open("pet-a", "小白");
    view.elements.breath.value = "3.2";
    view.elements.breath.dispatch("input");

    await view.control.save();

    expect(order).toEqual(["backend-save", "runtime-commit-3.25"]);
    expect(view.elements.breath.value).toBe("3.25");
    expect(view.elements.status.textContent).toContain("已保存");
  });

  it("settles a deferred backend save through canonical commit before close", async () => {
    const backend = deferred<PetCalibrationV1>();
    const canonical = calibration({ breathAmplitudePercent: 3.25 });
    const order: string[] = [];
    const view = mount({
      save: async () => { order.push("backend-start"); return backend.promise; },
      runtime: async (_petId, action, value) => { order.push(`${action}-${value.breathAmplitudePercent}`); },
    });
    await view.control.open("pet-a", "小白");
    view.elements.breath.value = "3.2";
    const saving = view.control.save();
    await view.settle();

    view.control.freezeForClose();
    let settled = false;
    const closing = view.control.settleForClose().then(() => { settled = true; });
    await view.control.save();
    expect(order).toEqual(["backend-start"]);
    expect(settled).toBe(false);

    backend.resolve(canonical);
    await saving;
    await closing;
    expect(order).toEqual(["backend-start", "commit-3.25"]);
    expect(view.control.needsRestoreBeforeClose()).toBe(false);
  });

  it("waits for a deferred canonical commit before close settles", async () => {
    const commit = deferred<void>();
    const canonical = calibration({ feedbackStrength: 0.8 });
    const order: string[] = [];
    const view = mount({
      save: async () => { order.push("backend-save"); return canonical; },
      runtime: async (_petId, action) => {
        order.push(action);
        if (action === "commit") return commit.promise;
      },
    });
    await view.control.open("pet-a", "小白");
    const saving = view.control.save();
    await view.settle();
    view.control.freezeForClose();
    let settled = false;
    const closing = view.control.settleForClose().then(() => { settled = true; });

    expect(order).toEqual(["backend-save", "commit"]);
    expect(settled).toBe(false);
    commit.resolve();
    await saving;
    await closing;
    expect(settled).toBe(true);
    expect(view.control.needsRestoreBeforeClose()).toBe(false);
  });

  it("settles a backend save failure by restoring the old saved value", async () => {
    const backend = deferred<PetCalibrationV1>();
    const oldSaved = calibration({ feedbackStrength: 0.4 });
    const actions: Array<{ action: string; value: PetCalibrationV1 }> = [];
    const view = mount({
      saved: oldSaved,
      save: () => backend.promise,
      runtime: async (_petId, action, value) => { actions.push({ action, value }); },
    });
    await view.control.open("pet-a", "小白");
    const saving = view.control.save();
    view.control.freezeForClose();
    const closing = view.control.settleForClose();

    backend.reject(new Error("disk full"));
    await saving;
    await closing;

    expect(actions).toEqual([{ action: "restore", value: oldSaved }]);
    expect(view.control.needsRestoreBeforeClose()).toBe(false);
  });

  it("settles through reload and retry commit without restoring stale saved state", async () => {
    const before = calibration({ feedbackStrength: 0.4 });
    const canonical = calibration({ feedbackStrength: 0.9 });
    const retry = deferred<void>();
    const actions: Array<{ action: string; value: PetCalibrationV1 }> = [];
    let commits = 0;
    const load = vi.fn().mockResolvedValueOnce(before).mockResolvedValueOnce(canonical);
    const view = mount({
      load,
      save: async () => canonical,
      runtime: async (_petId, action, value) => {
        actions.push({ action, value });
        if (action === "commit" && ++commits === 1) throw new Error("first commit failed");
        if (action === "commit") return retry.promise;
      },
    });
    await view.control.open("pet-a", "小白");
    const saving = view.control.save();
    await view.settle();
    view.control.freezeForClose();
    let settled = false;
    const closing = view.control.settleForClose().then(() => { settled = true; });
    await view.settle();

    expect(load).toHaveBeenCalledTimes(2);
    expect(actions).toEqual([
      { action: "commit", value: canonical },
      { action: "commit", value: canonical },
    ]);
    expect(settled).toBe(false);
    retry.resolve();
    await saving;
    await closing;
    expect(actions.some(({ action }) => action === "restore")).toBe(false);
    expect(view.control.needsRestoreBeforeClose()).toBe(false);
  });

  it("keeps backend canonical truth after reconcile fails and commits it on close", async () => {
    const before = calibration({ feedbackStrength: 0.4 });
    const canonical = calibration({ feedbackStrength: 0.9 });
    const actions: Array<{ action: string; value: PetCalibrationV1 }> = [];
    let commits = 0;
    const load = vi.fn().mockResolvedValueOnce(before).mockResolvedValueOnce(canonical);
    const view = mount({
      load,
      save: async () => canonical,
      runtime: async (_petId, action, value) => {
        actions.push({ action, value });
        if (action === "commit" && ++commits < 3) throw new Error(`commit ${commits} failed`);
      },
    });
    await view.control.open("pet-a", "小白");
    await view.control.save();

    expect(view.control.needsRestoreBeforeClose()).toBe(true);
    await view.control.restoreBeforeClose();

    expect(actions).toEqual([
      { action: "commit", value: canonical },
      { action: "commit", value: canonical },
      { action: "commit", value: canonical },
    ]);
    expect(actions.some(({ action }) => action === "restore")).toBe(false);
    expect(view.control.needsRestoreBeforeClose()).toBe(false);
  });

  it("does not carry a pending canonical commit into the next pet", async () => {
    const petACanonical = calibration({ feedbackStrength: 0.9 });
    const petBSaved = calibration({ feedbackStrength: 0.3 });
    const actions: Array<{ petId: string; action: string; value: PetCalibrationV1 }> = [];
    let petACommits = 0;
    const view = mount({
      load: vi.fn()
        .mockResolvedValueOnce(calibration({ feedbackStrength: 0.4 }))
        .mockResolvedValueOnce(petACanonical)
        .mockResolvedValueOnce(petBSaved),
      save: async () => petACanonical,
      runtime: async (petId, action, value) => {
        actions.push({ petId, action, value });
        if (petId === "pet-a" && action === "commit" && ++petACommits <= 2) {
          throw new Error("pet-a commit failed");
        }
      },
    });
    await view.control.open("pet-a", "小白");
    await view.control.save();
    expect(view.control.needsRestoreBeforeClose()).toBe(true);

    await view.control.open("pet-b", "小黑");
    expect(view.control.needsRestoreBeforeClose()).toBe(false);

    expect(actions.some(({ petId, action }) => petId === "pet-b" && action === "commit")).toBe(false);
  });

  it("does not commit and restores the previous snapshot when backend save fails", async () => {
    const oldSaved = calibration({ feedbackStrength: 0.4 });
    const view = mount({
      saved: oldSaved,
      save: async () => { throw new Error("disk full"); },
    });
    await view.control.open("pet-a", "小白");
    view.elements.feedback.value = "1";
    view.elements.feedback.dispatch("input");

    await view.control.save();

    expect(view.runtimeActions).toEqual([{ petId: "pet-a", action: "restore", value: oldSaved }]);
    expect(view.runtimeActions.some(({ action }) => action === "commit")).toBe(false);
    expect(view.elements.feedback.value).toBe("0.4");
    expect(view.elements.error.textContent).toContain("disk full");
  });

  it("restores promptly after save failure without waiting for a stale preview request", async () => {
    const preview = deferred<void>();
    const actions: string[] = [];
    const oldSaved = calibration({ feedbackStrength: 0.4 });
    const view = mount({
      saved: oldSaved,
      save: async () => { throw new Error("disk full"); },
      runtime: async (_petId, action) => {
        actions.push(action);
        if (action === "preview") return preview.promise;
      },
    });
    await view.control.open("pet-a", "小白");
    view.elements.feedback.value = "1";
    view.elements.feedback.dispatch("input");
    view.clock.flush();
    await view.settle();
    expect(actions).toEqual(["preview"]);

    let settled = false;
    const saving = view.control.save().then(() => { settled = true; });
    await view.settle();

    expect(actions).toEqual(["preview", "restore"]);
    expect(settled).toBe(true);
    expect(view.elements.feedback.value).toBe("0.4");
    preview.resolve();
    await saving;
  });

  it("reloads canonical state and retries commit after post-save commit failure", async () => {
    const before = calibration({ blinkIntervalScale: 0.8 });
    const canonical = calibration({ blinkIntervalScale: 1.3 });
    const load = vi.fn()
      .mockResolvedValueOnce(before)
      .mockResolvedValueOnce(canonical);
    let commitAttempts = 0;
    const committed: PetCalibrationV1[] = [];
    const view = mount({
      load,
      save: async () => canonical,
      runtime: async (_petId, action, value) => {
        if (action === "commit") committed.push(value);
        if (action === "commit" && ++commitAttempts === 1) throw new Error("pet busy");
      },
    });
    await view.control.open("pet-a", "小白");

    await view.control.save();

    expect(load).toHaveBeenCalledTimes(2);
    expect(commitAttempts).toBe(2);
    expect(committed.at(-1)?.blinkIntervalScale).toBe(1.3);
    expect(view.elements.status.textContent).toContain("已保存");
  });

  it("reports persisted state accurately if compensating commit also fails", async () => {
    const canonical = calibration({ feedbackStrength: 0.9 });
    const view = mount({
      load: async () => canonical,
      save: async () => canonical,
      runtime: async (_petId, action) => {
        if (action === "commit") throw new Error("pet window missing");
      },
    });
    await view.control.open("pet-a", "小白");

    await view.control.save();

    expect(view.elements.feedback.value).toBe("0.9");
    expect(view.elements.status.textContent).toContain("已保存");
    expect(view.elements.error.textContent).toContain("桌面预览尚未同步");
  });

  it("restores on cancel and when switching pets while ignoring a late old load", async () => {
    const petALoad = deferred<PetCalibrationV1>();
    const petB = calibration({ breathAmplitudePercent: 4 });
    const load = vi.fn((petId: string) => petId === "pet-a" ? petALoad.promise : Promise.resolve(petB));
    const view = mount({ load });
    const openingA = view.control.open("pet-a", "小白");
    await view.control.open("pet-b", "小黑");
    petALoad.resolve(calibration({ breathAmplitudePercent: 1 }));
    await openingA;

    expect(view.runtimeActions).toEqual([]);
    expect(view.elements.breath.value).toBe("4");
    expect(view.elements.petName.textContent).toBe("小黑");

    await view.control.cancel();
    expect(view.runtimeActions.at(-1)).toEqual({ petId: "pet-b", action: "restore", value: petB });
  });

  it("does not let a late cancel result overwrite newer input status", async () => {
    const restore = deferred<void>();
    const saved = calibration({ feedbackStrength: 0.4 });
    const view = mount({
      saved,
      runtime: async (_petId, action) => {
        if (action === "restore") return restore.promise;
      },
    });
    await view.control.open("pet-a", "小白");
    const cancelling = view.control.cancel();
    view.elements.feedback.value = "0.9";
    view.elements.feedback.dispatch("input");

    restore.resolve();
    await cancelling;

    expect(view.elements.feedback.value).toBe("0.9");
    expect(view.elements.status.textContent).toContain("正在准备预览");
  });

  it("clears timers and listeners on destroy and restores the current saved snapshot", async () => {
    const saved = calibration({ feedbackStrength: 0.5 });
    const view = mount({ saved });
    await view.control.open("pet-a", "小白");
    view.elements.feedback.value = "0.8";
    view.elements.feedback.dispatch("input");

    view.control.destroy();
    view.clock.flush();
    await view.settle();

    expect(view.clock.pendingCount).toBe(0);
    expect(view.runtimeActions).toEqual([{ petId: "pet-a", action: "restore", value: saved }]);
    for (const element of Object.values(view.elements)) expect(element.listenerCount()).toBe(0);
  });

  it("makes the panel unavailable when the catalog loses its current pet", async () => {
    const saved = calibration({ feedbackStrength: 0.5 });
    const view = mount({ saved });
    await view.control.open("pet-a", "小白");
    view.elements.feedback.value = "0.8";
    view.elements.feedback.dispatch("input");

    view.control.closeCurrent("当前宠物暂不可用");
    view.clock.flush();
    await view.settle();

    expect(view.elements.save.disabled).toBe(true);
    expect(view.elements.feedback.disabled).toBe(true);
    expect(view.elements.error.textContent).toContain("当前宠物暂不可用");
    expect(view.runtimeActions).toEqual([{ petId: "pet-a", action: "restore", value: saved }]);
  });
});

describe("PetCalibrationCatalogCoordinator", () => {
  it("does not cache a failed pet and retries it on the next catalog render", async () => {
    const target = {
      open: vi.fn()
        .mockResolvedValueOnce(false)
        .mockResolvedValueOnce(true),
      closeCurrent: vi.fn(),
      updatePetName: vi.fn(),
    };
    const coordinator = new PetCalibrationCatalogCoordinator(target);
    const entries = [{ petId: "pet-a", displayName: "小白", isCurrent: true }];

    await coordinator.reconcile(entries);
    await coordinator.reconcile(entries);

    expect(target.open).toHaveBeenCalledTimes(2);
    expect(target.updatePetName).toHaveBeenCalledWith("小白");
  });

  it("closes the old target on an empty or failed catalog and reloads when it returns", async () => {
    const target = {
      open: vi.fn().mockResolvedValue(true),
      closeCurrent: vi.fn(),
      updatePetName: vi.fn(),
    };
    const coordinator = new PetCalibrationCatalogCoordinator(target);
    const entries = [{ petId: "pet-a", displayName: "小白", isCurrent: true }];
    await coordinator.reconcile(entries);

    await coordinator.reconcile([]);
    coordinator.unavailable("目录读取失败");
    await coordinator.reconcile(entries);

    expect(target.closeCurrent).toHaveBeenNthCalledWith(1, "没有可校准的当前宠物");
    expect(target.closeCurrent).toHaveBeenNthCalledWith(2, "目录读取失败");
    expect(target.open).toHaveBeenCalledTimes(2);
  });
});
