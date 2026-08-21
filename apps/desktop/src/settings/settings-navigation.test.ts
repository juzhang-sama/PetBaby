import { describe, expect, it, vi } from "vitest";
import { initializeSettingsNavigation } from "./settings-navigation";

describe("initializeSettingsNavigation", () => {
  it("registers first and consumes navigation published before readiness", async () => {
    let handler: ((payload: unknown) => void) | undefined;
    const pending = ["calibration", null];
    const focus = vi.fn();

    const lifecycle = await initializeSettingsNavigation({
      listen: async (next) => {
        handler = next;
        next({ section: "calibration" });
        return () => undefined;
      },
      takePending: async () => pending.shift() ?? null,
      focusCalibration: focus,
    });

    expect(handler).toBeTypeOf("function");
    expect(focus).toHaveBeenCalledOnce();
    lifecycle.destroy();
  });

  it("consumes and clears every navigation while an existing page is ready", async () => {
    let handler!: (payload: unknown) => void;
    const pending: Array<string | null> = [null];
    const focus = vi.fn();
    const unlisten = vi.fn();
    const lifecycle = await initializeSettingsNavigation({
      listen: async (next) => { handler = next; return unlisten; },
      takePending: async () => pending.shift() ?? null,
      focusCalibration: focus,
    });
    pending.push("calibration", "calibration");

    handler({ section: "calibration" });
    handler({ section: "calibration" });
    for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();

    expect(focus).toHaveBeenCalledTimes(2);
    expect(pending).toEqual([]);
    lifecycle.destroy();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});
