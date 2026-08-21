import { describe, expect, it, vi } from "vitest";
import {
  createWindowModeClient,
  isWindowModeSnapshot,
  type WindowModeSnapshot,
} from "./window-mode-client";

const companion = (): WindowModeSnapshot => ({
  revision: 1,
  desiredMode: "companion",
  actualMode: "companion",
  desktopStrategy: null,
  userVisible: true,
  suppressions: [],
});

describe("window mode snapshot contract", () => {
  it("accepts the exact Rust snapshot including honest unknown actual state", () => {
    expect(isWindowModeSnapshot(companion())).toBe(true);
    expect(isWindowModeSnapshot({
      revision: 2,
      desiredMode: "desktop",
      actualMode: null,
      desktopStrategy: null,
      userVisible: false,
      suppressions: ["explorerLost", "transition"],
    })).toBe(true);
  });

  it("fails closed for unknown fields, enum values and inconsistent strategies", () => {
    expect(isWindowModeSnapshot({ ...companion(), extra: true })).toBe(false);
    expect(isWindowModeSnapshot({ ...companion(), suppressions: ["unknown"] })).toBe(false);
    expect(isWindowModeSnapshot({ ...companion(), actualMode: "desktopish" })).toBe(false);
    expect(isWindowModeSnapshot({ ...companion(), desktopStrategy: "workerW" })).toBe(false);
    expect(isWindowModeSnapshot({
      ...companion(),
      actualMode: "desktop",
      desiredMode: "desktop",
    })).toBe(false);
    expect(isWindowModeSnapshot({
      ...companion(),
      actualMode: null,
      desiredMode: "desktop",
      desktopStrategy: "bottomFallback",
    })).toBe(false);
    expect(isWindowModeSnapshot({
      ...companion(),
      actualMode: "desktop",
      desktopStrategy: "bottomFallback",
    })).toBe(true);
    expect(isWindowModeSnapshot({ ...companion(), revision: -1 })).toBe(false);
    expect(isWindowModeSnapshot({ ...companion(), revision: 1.5 })).toBe(false);
    expect(isWindowModeSnapshot({
      ...companion(),
      revision: Number.MAX_SAFE_INTEGER + 1,
    })).toBe(false);
  });

  it("subscribes to exact canonical snapshots and fails closed on invalid events", async () => {
    let handler!: (payload: unknown) => void;
    const unlisten = vi.fn();
    const client = createWindowModeClient({
      invoke: vi.fn(),
      listen: async (event, next) => {
        expect(event).toBe("window-mode:changed");
        handler = next;
        return unlisten;
      },
    });
    const snapshots: WindowModeSnapshot[] = [];
    const invalid = vi.fn();
    const unsubscribe = await client.subscribe((value) => snapshots.push(value), invalid);

    handler(companion());
    handler({ ...companion(), futureField: true });
    expect(snapshots).toEqual([companion()]);
    expect(invalid).toHaveBeenCalledOnce();
    unsubscribe();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});

describe("WindowModeClient", () => {
  it("uses one safe request id and rejects an invalid backend snapshot", async () => {
    const invoke = vi.fn()
      .mockResolvedValueOnce(companion())
      .mockResolvedValueOnce({ ...companion(), unknown: true });
    const client = createWindowModeClient({
      invoke,
      createRequestId: () => "settings-mode-1",
    });

    await expect(client.get()).resolves.toEqual(companion());
    await expect(client.set("desktop")).rejects.toThrow("无效的窗口模式状态");
    expect(invoke).toHaveBeenNthCalledWith(1, "window_mode_get");
    expect(invoke).toHaveBeenNthCalledWith(2, "window_mode_set", {
      requestId: "settings-mode-1",
      mode: "desktop",
    });
  });

  it("fails before invoking when the request id is unsafe", async () => {
    const invoke = vi.fn();
    const client = createWindowModeClient({ invoke, createRequestId: () => "bad id" });

    await expect(client.set("desktop")).rejects.toThrow("requestId");
    expect(invoke).not.toHaveBeenCalled();
  });
});
