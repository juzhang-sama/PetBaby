import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import type { WindowMode, WindowModeSnapshot } from "../runtime/window-mode-client";
import {
  WindowModeControl,
  type WindowModeControlElements,
} from "./window-mode-control";

type Listener = (event: { currentTarget?: FakeElement }) => void;

class FakeElement {
  value = "";
  textContent = "";
  checked = false;
  disabled = false;
  hidden = false;
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

  setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  removeAttribute(name: string): void { this.attributes.delete(name); }

  dispatch(type: string): void {
    for (const listener of this.listeners.get(type) ?? []) listener({ currentTarget: this });
  }
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

const snapshot = (
  actualMode: WindowMode | null,
  desktopStrategy: WindowModeSnapshot["desktopStrategy"] = null,
  suppressions: WindowModeSnapshot["suppressions"] = [],
  revision = 1,
): WindowModeSnapshot => ({
  revision,
  desiredMode: actualMode ?? "desktop",
  actualMode,
  desktopStrategy,
  userVisible: true,
  suppressions,
});

function mount(options: {
  initial?: Promise<WindowModeSnapshot>;
  set?: (mode: WindowMode) => Promise<WindowModeSnapshot>;
  subscribe?: (
    onSnapshot: (value: WindowModeSnapshot) => void,
  ) => Promise<() => void>;
} = {}) {
  const companion = new FakeElement();
  companion.value = "companion";
  const desktop = new FakeElement();
  desktop.value = "desktop";
  const root = new FakeElement();
  const status = new FakeElement();
  const error = new FakeElement();
  const compatibility = new FakeElement();
  const retry = new FakeElement();
  let eventHandler: ((value: WindowModeSnapshot) => void) | undefined;
  const diagnose = vi.fn();
  const client = {
    get: vi.fn(() => options.initial ?? Promise.resolve(snapshot("companion"))),
    set: vi.fn(options.set ?? (async (mode) => snapshot(mode, mode === "desktop" ? "workerW" : null, [], 2))),
    subscribe: vi.fn(options.subscribe ?? (async (onSnapshot: (value: WindowModeSnapshot) => void) => {
      eventHandler = onSnapshot;
      return () => { eventHandler = undefined; };
    })),
  };
  const control = new WindowModeControl({
    client,
    diagnose,
    elements: {
      root,
      choices: [companion, desktop],
      status,
      error,
      compatibility,
      retry,
    } as unknown as WindowModeControlElements,
  });
  return {
    control, client, diagnose, companion, desktop, root, status, error, compatibility, retry,
    publish: (value: WindowModeSnapshot) => eventHandler?.(value),
  };
}

describe("WindowModeControl", () => {
  it("keeps companion selected when desktop transition fails and permits retry", async () => {
    const set = vi.fn()
      .mockRejectedValueOnce(new Error("WorkerW and fallback failed"))
      .mockResolvedValueOnce(snapshot("desktop", "workerW", [], 2));
    const view = mount({ set });
    await view.control.mount();

    await view.control.choose("desktop");
    expect(view.companion.checked).toBe(true);
    expect(view.desktop.checked).toBe(false);
    expect(view.error.textContent).toContain("WorkerW and fallback failed");
    expect(view.desktop.disabled).toBe(false);

    await view.control.choose("desktop");
    expect(view.desktop.checked).toBe(true);
    expect(set).toHaveBeenCalledTimes(2);
  });

  it("disables both choices for the whole request and trusts only the returned actual snapshot", async () => {
    const pending = deferred<WindowModeSnapshot>();
    const view = mount({ set: () => pending.promise });
    await view.control.mount();
    const changing = view.control.choose("desktop");

    expect(view.companion.disabled).toBe(true);
    expect(view.desktop.disabled).toBe(true);
    expect(view.companion.checked).toBe(true);
    pending.resolve({ ...snapshot("companion", null, [], 2), desiredMode: "desktop" });
    await changing;

    expect(view.companion.checked).toBe(true);
    expect(view.desktop.checked).toBe(false);
  });

  it("restores the canonical radio before awaiting a change request", async () => {
    const pending = deferred<WindowModeSnapshot>();
    const view = mount({ set: () => pending.promise });
    await view.control.mount();

    view.companion.checked = false;
    view.desktop.checked = true;
    view.desktop.dispatch("change");

    expect(view.companion.checked).toBe(true);
    expect(view.desktop.checked).toBe(false);
    expect(view.companion.disabled).toBe(true);
    expect(view.desktop.disabled).toBe(true);
    pending.resolve(snapshot("desktop", "workerW", [], 2));
    await Promise.resolve();
  });

  it("shows the compatibility note for BottomFallback", async () => {
    const view = mount({ set: async () => snapshot("desktop", "bottomFallback", [], 2) });
    await view.control.mount();
    await view.control.choose("desktop");

    expect(view.compatibility.textContent).toBe("已使用兼容桌面层");
    expect(view.compatibility.hidden).toBe(false);
  });

  it("leaves both choices unselected and reports an honest degraded state", async () => {
    const view = mount({ initial: Promise.resolve(snapshot(null, null, ["transition"])) });
    await view.control.mount();

    expect(view.companion.checked).toBe(false);
    expect(view.desktop.checked).toBe(false);
    expect(view.error.textContent).toContain("无法确认");
  });

  it("keeps controls disabled after load failure until retry succeeds", async () => {
    const clientGet = vi.fn()
      .mockRejectedValueOnce(new Error("pet unavailable"))
      .mockResolvedValueOnce(snapshot("companion"));
    const view = mount();
    view.client.get = clientGet;
    await view.control.mount();

    expect(view.companion.disabled).toBe(true);
    expect(view.desktop.disabled).toBe(true);
    expect(view.retry.hidden).toBe(false);
    expect(view.error.textContent).toContain("pet unavailable");

    view.retry.dispatch("click");
    await Promise.resolve();
    await Promise.resolve();
    expect(view.companion.checked).toBe(true);
    expect(view.companion.disabled).toBe(false);
  });

  it("ignores late load and mutation results after destroy", async () => {
    const loading = deferred<WindowModeSnapshot>();
    const view = mount({ initial: loading.promise });
    const mounted = view.control.mount();
    view.control.destroy();
    loading.resolve(snapshot("companion"));
    await mounted;
    expect(view.companion.checked).toBe(false);

    const changing = deferred<WindowModeSnapshot>();
    const second = mount({ set: () => changing.promise });
    await second.control.mount();
    const result = second.control.choose("desktop");
    const before = second.status.textContent;
    second.control.destroy();
    changing.resolve(snapshot("desktop", "workerW"));
    await result;
    expect(second.desktop.checked).toBe(false);
    expect(second.status.textContent).toBe(before);
  });

  it("releases a snapshot subscription that resolves after destroy", async () => {
    const pending = deferred<() => void>();
    const unlisten = vi.fn();
    const view = mount({ subscribe: async () => pending.promise });
    const mounted = view.control.mount();

    view.control.destroy();
    pending.resolve(unlisten);
    await mounted;

    expect(unlisten).toHaveBeenCalledOnce();
    expect(view.client.get).not.toHaveBeenCalled();
  });

  it("converges to a newer canonical event and ignores the older request result", async () => {
    const changing = deferred<WindowModeSnapshot>();
    const view = mount({ set: () => changing.promise });
    await view.control.mount();
    const result = view.control.choose("desktop");

    view.publish(snapshot("desktop", "bottomFallback", [], 2));
    expect(view.desktop.checked).toBe(true);
    expect(view.compatibility.textContent).toBe("已使用兼容桌面层");
    changing.resolve(snapshot("companion"));
    await result;

    expect(view.desktop.checked).toBe(true);
    expect(view.companion.checked).toBe(false);
  });

  it("ignores delayed older events and results after a newer revision", async () => {
    const changing = deferred<WindowModeSnapshot>();
    const view = mount({
      initial: Promise.resolve(snapshot("companion", null, [], 1)),
      set: () => changing.promise,
    });
    await view.control.mount();
    const result = view.control.choose("desktop");

    view.publish(snapshot("desktop", "workerW", [], 3));
    view.publish(snapshot("companion", null, [], 2));
    changing.resolve(snapshot("companion", null, [], 2));
    await result;

    expect(view.desktop.checked).toBe(true);
    expect(view.companion.checked).toBe(false);
  });

  it("fails closed and reloads when one revision carries conflicting payloads", async () => {
    const view = mount({ initial: Promise.resolve(snapshot("companion", null, [], 7)) });
    await view.control.mount();

    view.publish(snapshot("desktop", "workerW", [], 7));
    await Promise.resolve();

    expect(view.companion.checked).toBe(true);
    expect(view.desktop.checked).toBe(false);
    expect(view.diagnose).toHaveBeenCalledOnce();
    expect(view.client.get).toHaveBeenCalledTimes(2);
  });

  it("continues initial loading when realtime subscription is unavailable", async () => {
    const view = mount({
      subscribe: async () => { throw new Error("event bus unavailable"); },
    });

    await expect(view.control.mount()).resolves.toBeUndefined();

    expect(view.client.get).toHaveBeenCalledOnce();
    expect(view.companion.checked).toBe(true);
    expect(view.companion.disabled).toBe(false);
    expect(view.error.textContent).toContain("实时同步不可用");
    expect(view.diagnose).toHaveBeenCalledOnce();
  });

  it("refreshes from backend on pageshow and removes lifecycle listeners on destroy", async () => {
    class FakePage {
      private handler?: () => void;
      addEventListener(_: string, handler: () => void): void { this.handler = handler; }
      removeEventListener(_: string, handler: () => void): void {
        if (handler === this.handler) this.handler = undefined;
      }
      show(): void { this.handler?.(); }
    }
    const page = new FakePage();
    const view = mount();
    view.control.attachPageLifecycle(page as unknown as Window);
    await view.control.mount();
    page.show();
    await Promise.resolve();
    expect(view.client.get).toHaveBeenCalledTimes(2);
    view.control.destroy();
    page.show();
    await Promise.resolve();
    expect(view.client.get).toHaveBeenCalledTimes(2);
  });
});

describe("window mode settings assembly", () => {
  it("does not expose the retired dual-mode selector", () => {
    const html = readFileSync(new URL("../../settings.html", import.meta.url), "utf8");
    const source = readFileSync(new URL("../settings.ts", import.meta.url), "utf8");

    expect(html).not.toContain('id="window-mode-section"');
    expect(html).not.toContain('name="window-mode"');
    expect(source).not.toContain("createWindowModeClient");
    expect(source).not.toContain("new WindowModeControl");
  });
});
