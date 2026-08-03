import { describe, expect, it, vi } from "vitest";
import { RenderScheduler } from "./render-scheduler";

describe("RenderScheduler", () => {
  it("stops continuous rendering in still and paused tiers", () => {
    const start = vi.fn();
    const stop = vi.fn();
    const scheduler = new RenderScheduler({ start, stop, setMaxFps: vi.fn(), renderOnce: vi.fn() });
    scheduler.setTier("still");
    scheduler.setTier("paused");
    expect(stop).toHaveBeenCalledTimes(2);
    expect(start).not.toHaveBeenCalled();
  });

  it("uses 24 fps for companion and 60 fps for temporary active interaction", () => {
    const setMaxFps = vi.fn();
    const scheduler = new RenderScheduler({ start: vi.fn(), stop: vi.fn(), setMaxFps, renderOnce: vi.fn() });
    scheduler.setTier("companion");
    scheduler.setTier("active");
    expect(setMaxFps.mock.calls).toEqual([[24], [60]]);
  });

  it("keeps the ticker running for companion so animations advance", () => {
    const start = vi.fn();
    const scheduler = new RenderScheduler({ start, stop: vi.fn(), setMaxFps: vi.fn(), renderOnce: vi.fn() });
    scheduler.setTier("companion");
    expect(start).toHaveBeenCalledTimes(1);
  });
});
