import { describe, expect, it, vi } from "vitest";
import {
  classifyFullscreen,
  isFullscreenSnapshot,
  wireFullscreenProbeLoop,
  type FullscreenSnapshot,
} from "./fullscreen";

describe("classifyFullscreen", () => {
  const monitor = { left: 0, top: 0, right: 1920, bottom: 1080 };

  it("accepts a borderless window within two pixels", () => {
    expect(classifyFullscreen({ left: -1, top: 0, right: 1921, bottom: 1080 }, monitor, 2)).toBe(true);
  });

  it("rejects a maximized work-area window that leaves the taskbar visible", () => {
    expect(classifyFullscreen({ left: 0, top: 0, right: 1920, bottom: 1040 }, monitor, 2)).toBe(false);
  });

  it("accepts the exact other-monitor diagnostic and rejects unknown reasons", () => {
    expect(isFullscreenSnapshot({
      isFullscreen: false,
      foregroundHwnd: 42,
      monitorRect: { left: 1920, top: 0, right: 3840, bottom: 1080 },
      reason: "fullscreen-on-other-monitor",
    })).toBe(true);
    expect(isFullscreenSnapshot({
      isFullscreen: false,
      foregroundHwnd: 42,
      monitorRect: null,
      reason: "other-monitor",
    })).toBe(false);
  });

  it("keeps one update in flight and coalesces interval ticks to the latest fact", async () => {
    const facts: FullscreenSnapshot[] = [
      { isFullscreen: true, foregroundHwnd: 1, monitorRect: null, reason: "foreground-covers-monitor" },
      { isFullscreen: true, foregroundHwnd: 2, monitorRect: null, reason: "foreground-covers-monitor" },
      { isFullscreen: false, foregroundHwnd: 3, monitorRect: null, reason: "not-fullscreen" },
    ];
    let tick!: () => void;
    let finishFirst!: () => void;
    const firstUpdate = new Promise<void>((resolve) => { finishFirst = resolve; });
    const update = vi.fn()
      .mockImplementationOnce(() => firstUpdate)
      .mockResolvedValueOnce(undefined);
    const reconcile = vi.fn(async () => undefined);
    const clearInterval = vi.fn();
    const wiring = wireFullscreenProbeLoop({
      setInterval: (handler) => { tick = handler; return 17; },
      clearInterval,
      probe: vi.fn(async () => facts.shift()!),
      update,
      reconcile,
      diagnose: vi.fn(),
    });

    tick();
    await vi.waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(reconcile).not.toHaveBeenCalled();
    tick();
    tick();
    await Promise.resolve();
    await Promise.resolve();
    expect(update).toHaveBeenCalledTimes(1);

    finishFirst();
    await vi.waitFor(() => expect(update).toHaveBeenCalledTimes(2));
    expect(update.mock.calls.map(([snapshot]) => snapshot.isFullscreen)).toEqual([true, false]);
    expect(reconcile).toHaveBeenCalledTimes(2);
    wiring.destroy();
    wiring.destroy();
    expect(clearInterval).toHaveBeenCalledOnce();
    expect(clearInterval).toHaveBeenCalledWith(17);
  });

  it("keeps one probe in flight and ignores an older result after the newest tick", async () => {
    let tick!: () => void;
    let finishFirst!: (value: FullscreenSnapshot) => void;
    const firstProbe = new Promise<FullscreenSnapshot>((resolve) => { finishFirst = resolve; });
    const newest = { isFullscreen: false, foregroundHwnd: 2, monitorRect: null, reason: "not-fullscreen" } as const;
    const probe = vi.fn()
      .mockImplementationOnce(() => firstProbe)
      .mockResolvedValue(newest);
    const update = vi.fn(async () => undefined);
    const wiring = wireFullscreenProbeLoop({
      setInterval: (handler) => { tick = handler; return 23; },
      clearInterval: vi.fn(),
      probe,
      update,
      reconcile: vi.fn(async () => undefined),
    });

    tick();
    tick();
    tick();
    expect(probe).toHaveBeenCalledTimes(1);
    finishFirst({ isFullscreen: true, foregroundHwnd: 1, monitorRect: null, reason: "foreground-covers-monitor" });
    await vi.waitFor(() => expect(probe).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(update).toHaveBeenLastCalledWith(newest));
    expect(update).toHaveBeenCalledTimes(2);

    wiring.destroy();
  });

  it("does not publish a late probe result after destroy", async () => {
    let tick!: () => void;
    let finish!: (value: FullscreenSnapshot) => void;
    const probe = new Promise<FullscreenSnapshot>((resolve) => { finish = resolve; });
    const update = vi.fn(async () => undefined);
    const wiring = wireFullscreenProbeLoop({
      setInterval: (handler) => { tick = handler; return 29; },
      clearInterval: vi.fn(),
      probe: vi.fn(() => probe),
      update,
      reconcile: vi.fn(async () => undefined),
    });

    tick();
    wiring.destroy();
    finish({ isFullscreen: true, foregroundHwnd: 9, monitorRect: null, reason: "foreground-covers-monitor" });
    await Promise.resolve();
    await Promise.resolve();

    expect(update).not.toHaveBeenCalled();
  });
});
