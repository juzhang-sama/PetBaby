import { describe, expect, it, vi } from "vitest";
import { SettingsCloseCoordinator } from "./settings-close-coordinator";

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

class FakeClock {
  private nextId = 1;
  private callbacks = new Map<number, () => void>();
  setTimeout = (callback: () => void): number => {
    const id = this.nextId++;
    this.callbacks.set(id, callback);
    return id;
  };
  clearTimeout = (id: number): void => { this.callbacks.delete(id); };
  timeout(): void {
    const callbacks = [...this.callbacks.values()];
    this.callbacks.clear();
    for (const callback of callbacks) callback();
  }
}

function mount(options: {
  settle?: () => Promise<void>;
  restore?: () => Promise<void>;
  hasActive?: () => boolean;
  destroy?: () => Promise<void>;
} = {}) {
  let closeHandler: ((event: { preventDefault(): void }) => void) | undefined;
  const clock = new FakeClock();
  const destroy = vi.fn(options.destroy ?? (async () => undefined));
  const freeze = vi.fn();
  const unfreeze = vi.fn();
  const settleSave = vi.fn(options.settle ?? (async () => undefined));
  const cleanup = vi.fn();
  const diagnose = vi.fn();
  const restore = vi.fn(options.restore ?? (async () => undefined));
  const coordinator = new SettingsCloseCoordinator({
    onCloseRequested: async (handler) => {
      closeHandler = handler;
      return () => { closeHandler = undefined; };
    },
    destroy,
    freeze,
    unfreeze,
    settle: settleSave,
    restore,
    hasActive: options.hasActive ?? (() => true),
    cleanup,
    diagnose,
    clock,
    timeoutMs: 250,
  });
  return {
    coordinator, clock, destroy, freeze, unfreeze, settleSave, restore, cleanup, diagnose,
    async ready() { await coordinator.mount(); },
    requestClose() {
      let prevented = false;
      closeHandler?.({ preventDefault: () => { prevented = true; } });
      return prevented;
    },
    async settle() { for (let turn = 0; turn < 8; turn += 1) await Promise.resolve(); },
  };
}

describe("SettingsCloseCoordinator", () => {
  it("prevents close while restore is pending then closes and cleans exactly once after ACK", async () => {
    const restoring = deferred<void>();
    const view = mount({ restore: () => restoring.promise });
    await view.ready();

    expect(view.requestClose()).toBe(true);
    await view.settle();
    expect(view.freeze).toHaveBeenCalledOnce();
    expect(view.restore).toHaveBeenCalledOnce();
    expect(view.destroy).not.toHaveBeenCalled();
    expect(view.cleanup).not.toHaveBeenCalled();

    restoring.resolve();
    await view.settle();
    expect(view.destroy).toHaveBeenCalledOnce();
    expect(view.cleanup).toHaveBeenCalledOnce();
  });

  it("waits for an in-flight save before deciding whether restore is needed", async () => {
    const save = deferred<void>();
    let needsRestore = true;
    const view = mount({
      settle: async () => { await save.promise; needsRestore = false; },
      hasActive: () => needsRestore,
    });
    await view.ready();
    expect(view.requestClose()).toBe(true);
    await view.settle();

    expect(view.destroy).not.toHaveBeenCalled();
    expect(view.restore).not.toHaveBeenCalled();
    save.resolve();
    await view.settle();
    expect(view.restore).not.toHaveBeenCalled();
    expect(view.destroy).toHaveBeenCalledOnce();
  });

  it.each(["reject", "timeout"] as const)(
    "closes in finite time and diagnoses when restore ends by %s",
    async (ending) => {
      const restoring = deferred<void>();
      const view = mount({ restore: () => restoring.promise });
      await view.ready();
      expect(view.requestClose()).toBe(true);
      await view.settle();

      if (ending === "reject") restoring.reject(new Error("restore failed"));
      else view.clock.timeout();
      await view.settle();

      expect(view.diagnose).toHaveBeenCalledOnce();
      expect(view.destroy).toHaveBeenCalledOnce();
      expect(view.cleanup).toHaveBeenCalledOnce();
    },
  );

  it("is idempotent for repeated close requests", async () => {
    const restoring = deferred<void>();
    const view = mount({ restore: () => restoring.promise });
    await view.ready();

    expect(view.requestClose()).toBe(true);
    expect(view.requestClose()).toBe(true);
    await view.settle();
    expect(view.restore).toHaveBeenCalledOnce();
    restoring.resolve();
    await view.settle();
    expect(view.destroy).toHaveBeenCalledOnce();
    expect(view.cleanup).toHaveBeenCalledOnce();
  });

  it("closes quickly without restore when no calibration is active", async () => {
    const view = mount({ hasActive: () => false });
    await view.ready();

    expect(view.requestClose()).toBe(true);
    await view.settle();

    expect(view.restore).not.toHaveBeenCalled();
    expect(view.destroy).toHaveBeenCalledOnce();
    expect(view.cleanup).toHaveBeenCalledOnce();
  });

  it("destroys the settings WebView without cancelling an independent photo-avatar job", async () => {
    const backgroundJob = deferred<string>();
    const completed = vi.fn();
    void backgroundJob.promise.then(completed);
    const view = mount({ hasActive: () => false });
    await view.ready();

    expect(view.requestClose()).toBe(true);
    await view.settle();
    expect(view.destroy).toHaveBeenCalledOnce();

    backgroundJob.resolve("previewReady");
    await view.settle();
    expect(completed).toHaveBeenCalledWith("previewReady");
  });

  it("beforeunload only performs synchronous final cleanup and never sends restore again", async () => {
    const view = mount();
    await view.ready();

    view.coordinator.beforeUnload();

    expect(view.restore).not.toHaveBeenCalled();
    expect(view.cleanup).toHaveBeenCalledOnce();
  });

  it("keeps the listener and retries after native destroy rejects", async () => {
    const destroy = vi.fn()
      .mockRejectedValueOnce(new Error("native destroy rejected"))
      .mockResolvedValueOnce(undefined);
    const view = mount({ destroy, hasActive: () => false });
    await view.ready();

    expect(view.requestClose()).toBe(true);
    await view.settle();
    expect(view.diagnose).toHaveBeenCalledOnce();
    expect(view.cleanup).not.toHaveBeenCalled();
    expect(view.unfreeze).toHaveBeenCalledOnce();

    expect(view.requestClose()).toBe(true);
    await view.settle();
    expect(view.destroy).toHaveBeenCalledTimes(2);
    expect(view.restore).not.toHaveBeenCalled();
    expect(view.cleanup).toHaveBeenCalledOnce();
  });

  it("does not repeat a successful restore when native destroy is retried", async () => {
    let needsRestore = true;
    const destroy = vi.fn()
      .mockRejectedValueOnce(new Error("native destroy rejected"))
      .mockResolvedValueOnce(undefined);
    const view = mount({
      destroy,
      hasActive: () => needsRestore,
      restore: async () => { needsRestore = false; },
    });
    await view.ready();

    expect(view.requestClose()).toBe(true);
    await view.settle();
    expect(view.restore).toHaveBeenCalledOnce();
    expect(view.cleanup).not.toHaveBeenCalled();

    expect(view.requestClose()).toBe(true);
    await view.settle();
    expect(view.restore).toHaveBeenCalledOnce();
    expect(view.destroy).toHaveBeenCalledTimes(2);
    expect(view.cleanup).toHaveBeenCalledOnce();
  });

  it("keeps settings open past the timeout until backend save and canonical commit settle", async () => {
    const backend = deferred<{ breath: number }>();
    const commit = deferred<void>();
    const actions: string[] = [];
    let needsRestore = true;
    const view = mount({
      settle: async () => {
        actions.push("backend-save");
        const canonical = await backend.promise;
        actions.push(`commit-${canonical.breath}`);
        await commit.promise;
        needsRestore = false;
      },
      hasActive: () => needsRestore,
    });
    await view.ready();
    expect(view.requestClose()).toBe(true);
    await view.settle();

    view.clock.timeout();
    await view.settle();

    expect(view.diagnose).toHaveBeenCalledOnce();
    expect(String(view.diagnose.mock.calls[0]?.[0])).toContain("保存仍在进行，暂不能关闭");
    expect(view.destroy).not.toHaveBeenCalled();
    expect(view.cleanup).not.toHaveBeenCalled();
    expect(view.unfreeze).not.toHaveBeenCalled();

    expect(view.requestClose()).toBe(true);
    expect(view.settleSave).toHaveBeenCalledOnce();
    backend.resolve({ breath: 3.25 });
    await view.settle();
    expect(actions).toEqual(["backend-save", "commit-3.25"]);
    expect(view.destroy).not.toHaveBeenCalled();

    commit.resolve();
    await view.settle();
    expect(view.restore).not.toHaveBeenCalled();
    expect(view.destroy).toHaveBeenCalledOnce();
    expect(view.cleanup).toHaveBeenCalledOnce();
  });

  it("also waits without closing when canonical reload and retry commit exceed the timeout", async () => {
    const retryCommit = deferred<void>();
    const actions: string[] = [];
    let needsRestore = true;
    const view = mount({
      settle: async () => {
        actions.push("backend-canonical");
        actions.push("commit-first-failed");
        actions.push("reload-canonical");
        actions.push("commit-retry");
        await retryCommit.promise;
        needsRestore = false;
      },
      hasActive: () => needsRestore,
    });
    await view.ready();
    expect(view.requestClose()).toBe(true);
    await view.settle();

    view.clock.timeout();
    await view.settle();
    expect(actions).toEqual([
      "backend-canonical",
      "commit-first-failed",
      "reload-canonical",
      "commit-retry",
    ]);
    expect(view.destroy).not.toHaveBeenCalled();
    expect(view.cleanup).not.toHaveBeenCalled();

    retryCommit.resolve();
    await view.settle();
    expect(view.restore).not.toHaveBeenCalled();
    expect(view.destroy).toHaveBeenCalledOnce();
  });

  it("waits past the timeout for backend rejection and old-saved restore before closing", async () => {
    const backend = deferred<void>();
    const actions: string[] = [];
    let needsRestore = true;
    const view = mount({
      settle: async () => {
        try {
          await backend.promise;
        } catch {
          actions.push("restore-old-saved");
          needsRestore = false;
        }
      },
      hasActive: () => needsRestore,
    });
    await view.ready();
    expect(view.requestClose()).toBe(true);
    await view.settle();

    view.clock.timeout();
    await view.settle();
    expect(view.destroy).not.toHaveBeenCalled();
    expect(view.cleanup).not.toHaveBeenCalled();

    backend.reject(new Error("backend save failed"));
    await view.settle();
    expect(actions).toEqual(["restore-old-saved"]);
    expect(view.restore).not.toHaveBeenCalled();
    expect(view.destroy).toHaveBeenCalledOnce();
  });
});
