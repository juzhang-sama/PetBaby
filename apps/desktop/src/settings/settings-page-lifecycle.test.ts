import { describe, expect, it, vi } from "vitest";
import { wireSettingsPageLifecycle } from "./settings-page-lifecycle";

class FakeEvents {
  private listeners = new Map<string, Set<() => void>>();
  addEventListener(type: string, listener: () => void): void {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }
  removeEventListener(type: string, listener: () => void): void {
    this.listeners.get(type)?.delete(listener);
  }
  dispatch(type: string): void {
    for (const listener of this.listeners.get(type) ?? []) listener();
  }
}

describe("wireSettingsPageLifecycle", () => {
  it("restores on pagehide and reloads calibration on BFCache pageshow without destroying it", () => {
    const events = new FakeEvents();
    const suspend = vi.fn();
    const resume = vi.fn();
    const destroy = vi.fn();
    const lifecycle = wireSettingsPageLifecycle(events, { suspend, resume, destroy });

    events.dispatch("pagehide");
    events.dispatch("pageshow");

    expect(suspend).toHaveBeenCalledOnce();
    expect(resume).toHaveBeenCalledOnce();
    expect(destroy).not.toHaveBeenCalled();
    lifecycle.destroy();
  });

  it("permanently destroys only on beforeunload and removes all listeners", () => {
    const events = new FakeEvents();
    const ports = { suspend: vi.fn(), resume: vi.fn(), destroy: vi.fn() };
    const lifecycle = wireSettingsPageLifecycle(events, ports);

    events.dispatch("beforeunload");
    events.dispatch("pagehide");
    events.dispatch("pageshow");

    expect(ports.destroy).toHaveBeenCalledOnce();
    expect(ports.suspend).not.toHaveBeenCalled();
    expect(ports.resume).not.toHaveBeenCalled();
    lifecycle.destroy();
    expect(ports.destroy).toHaveBeenCalledOnce();
  });
});
