import { describe, expect, it, vi } from "vitest";
import {
  WindowSizeController,
  type WindowRect,
  type WindowSizeApplyError,
  type WindowSizePort,
} from "./window-size-controller";

type Stage = "getRect" | "getWorkArea" | "setRect" | "resizeRenderer" | "refreshHitRegion" | "readback";

interface HarnessOptions {
  failForward?: Stage;
  forwardError?: unknown;
  actualRect?: WindowRect;
  rollbackFailures?: Partial<Record<"setRect" | "resizeRenderer" | "refreshHitRegion", unknown>>;
}

function harness(options: HarnessOptions = {}) {
  const originalRect: WindowRect = { x: 1000, y: 400, width: 420, height: 520 };
  const workArea = { x: 0, y: 0, width: 1920, height: 1040 };
  const calls: string[] = [];
  let currentRect = { ...originalRect };
  let getRectCalls = 0;
  let setRectCalls = 0;
  const forwardError = Object.prototype.hasOwnProperty.call(options, "forwardError")
    ? options.forwardError
    : new Error(`${options.failForward ?? "forward"} failed`, { cause: new Error("native cause") });

  const port: WindowSizePort = {
    getRect: vi.fn(async () => {
      getRectCalls += 1;
      const readback = getRectCalls > 1;
      calls.push(readback ? "getRect:readback" : "getRect:old");
      if ((!readback && options.failForward === "getRect")
        || (readback && options.failForward === "readback")) throw forwardError;
      return { ...currentRect };
    }),
    getWorkArea: vi.fn(async () => {
      calls.push("getWorkArea");
      if (options.failForward === "getWorkArea") throw forwardError;
      return { ...workArea };
    }),
    setRect: vi.fn(async (rect) => {
      setRectCalls += 1;
      calls.push(setRectCalls === 1 ? "setRect:forward" : "setRect:rollback");
      if (setRectCalls === 1 && options.failForward === "setRect") throw forwardError;
      if (setRectCalls > 1 && options.rollbackFailures?.setRect) throw options.rollbackFailures.setRect;
      currentRect = setRectCalls === 1 && options.actualRect
        ? { ...options.actualRect }
        : { ...rect };
    }),
    resizeRenderer: vi.fn(async () => {
      const rollingBack = setRectCalls > 1;
      calls.push(rollingBack ? "resizeRenderer:rollback" : "resizeRenderer:forward");
      if (!rollingBack && options.failForward === "resizeRenderer") throw forwardError;
      if (rollingBack && options.rollbackFailures?.resizeRenderer) {
        throw options.rollbackFailures.resizeRenderer;
      }
    }),
    refreshHitRegion: vi.fn(async () => {
      const rollingBack = setRectCalls > 1;
      calls.push(rollingBack ? "refreshHitRegion:rollback" : "refreshHitRegion:forward");
      if (!rollingBack && options.failForward === "refreshHitRegion") throw forwardError;
      if (rollingBack && options.rollbackFailures?.refreshHitRegion) {
        throw options.rollbackFailures.refreshHitRegion;
      }
    }),
  };

  return { calls, currentRect: () => currentRect, forwardError, originalRect, port, workArea };
}

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("WindowSizeController", () => {
  it("applies the anchored rect in fixed order and returns its acknowledgement", async () => {
    const test = harness();

    const ack = await new WindowSizeController(test.port).apply(0.75);

    const targetRect = { x: 1052, y: 530, width: 315, height: 390 };
    expect(test.calls).toEqual([
      "getRect:old",
      "getWorkArea",
      "setRect:forward",
      "resizeRenderer:forward",
      "refreshHitRegion:forward",
      "getRect:readback",
    ]);
    expect(test.port.setRect).toHaveBeenCalledWith(targetRect);
    expect(ack).toEqual({ requestedScale: 0.75, appliedScale: 0.75, rect: targetRect });
  });

  it("acknowledges a finite fractional OS rect with its exact smaller scale", async () => {
    const actualRect = { x: 1050.5, y: 532.25, width: 211, height: 261 };
    const test = harness({ actualRect });

    const ack = await new WindowSizeController(test.port).apply(0.75);

    expect(ack).toEqual({
      requestedScale: 0.75,
      appliedScale: Math.min(211 / 420, 261 / 520),
      rect: actualRect,
    });
  });

  it.each([
    ["an undersized 100x100 size", { x: 0, y: 0, width: 100, height: 100 }],
    ["zero width", { x: 0, y: 0, width: 0, height: 520 }],
    ["negative width", { x: 0, y: 0, width: -1, height: 520 }],
    ["zero height", { x: 0, y: 0, width: 420, height: 0 }],
    ["negative height", { x: 0, y: 0, width: 420, height: -1 }],
    ["a NaN x coordinate", { x: Number.NaN, y: 0, width: 420, height: 520 }],
    ["an infinite y coordinate", { x: 0, y: Number.POSITIVE_INFINITY, width: 420, height: 520 }],
    ["a NaN width", { x: 0, y: 0, width: Number.NaN, height: 520 }],
    ["an infinite height", { x: 0, y: 0, width: 420, height: Number.POSITIVE_INFINITY }],
    ["an unsupported 631x781 size", { x: 0, y: 0, width: 631, height: 781 }],
  ] as const)("rolls back when readback contains %s", async (_case, actualRect) => {
    const test = harness({ actualRect });

    const error = await new WindowSizeController(test.port).apply(1.25).catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(RangeError);
    expect((error as Error).message).toContain("Invalid resized window acknowledgement");
    expect(test.port.setRect).toHaveBeenLastCalledWith(test.originalRect);
    expect(test.calls.slice(-3)).toEqual([
      "setRect:rollback",
      "resizeRenderer:rollback",
      "refreshHitRegion:rollback",
    ]);
  });

  it.each([
    [0.5, { x: 1105, y: 660, width: 210, height: 260 }],
    [1, { x: 1000, y: 400, width: 420, height: 520 }],
    [1.5, { x: 895, y: 140, width: 630, height: 780 }],
  ] as const)("applies scale %s in logical pixels", async (scale, expected) => {
    const test = harness();

    const ack = await new WindowSizeController(test.port).apply(scale);

    expect(ack.rect).toEqual(expected);
    expect(test.port.setRect).toHaveBeenCalledWith(expected);
  });

  it("runs the deterministic pipeline for an unchanged one-hundred-percent scale", async () => {
    const test = harness();

    const ack = await new WindowSizeController(test.port).apply(1);

    expect(ack.rect).toEqual(test.originalRect);
    expect(test.port.setRect).toHaveBeenCalledOnce();
    expect(test.port.resizeRenderer).toHaveBeenCalledOnce();
    expect(test.port.refreshHitRegion).toHaveBeenCalledOnce();
  });

  it("commits after readback and before acknowledging the request", async () => {
    const test = harness();
    const events = test.calls;

    const ack = await new WindowSizeController(test.port).apply(1.25, async (actual) => {
      events.push(`commit:${actual.rect.width}`);
    });

    expect(events).toEqual([
      "getRect:old",
      "getWorkArea",
      "setRect:forward",
      "resizeRenderer:forward",
      "refreshHitRegion:forward",
      "getRect:readback",
      "commit:525",
    ]);
    expect(ack.rect.width).toBe(525);
  });

  it("rolls the exact original rectangle and visual pipeline back when commit fails", async () => {
    const test = harness();
    const saveError = new Error("save failed");

    await expect(new WindowSizeController(test.port).apply(1.25, async () => {
      throw saveError;
    })).rejects.toBe(saveError);

    expect(test.port.setRect).toHaveBeenLastCalledWith(test.originalRect);
    expect(test.currentRect()).toEqual(test.originalRect);
    expect(test.calls.slice(-3)).toEqual([
      "setRect:rollback",
      "resizeRenderer:rollback",
      "refreshHitRegion:rollback",
    ]);
  });

  it("rolls back when a compound setRect port mutates size before rejecting position", async () => {
    const originalRect = { x: 1000, y: 400, width: 420, height: 520 };
    let currentRect = { ...originalRect };
    let setCalls = 0;
    const port: WindowSizePort = {
      getRect: vi.fn(async () => ({ ...currentRect })),
      getWorkArea: vi.fn(async () => ({ x: 0, y: 0, width: 1920, height: 1040 })),
      setRect: vi.fn(async (rect) => {
        setCalls += 1;
        if (setCalls === 1) {
          currentRect = { ...currentRect, width: rect.width, height: rect.height };
          throw new Error("position failed after size changed");
        }
        currentRect = { ...rect };
      }),
      resizeRenderer: vi.fn(async () => undefined),
      refreshHitRegion: vi.fn(async () => undefined),
    };

    await expect(new WindowSizeController(port).apply(1.25)).rejects.toThrow("position failed");

    expect(port.setRect).toHaveBeenCalledTimes(2);
    expect(currentRect).toEqual(originalRect);
    expect(port.resizeRenderer).toHaveBeenCalledOnce();
    expect(port.refreshHitRegion).toHaveBeenCalledOnce();
  });

  it.each(["getRect", "getWorkArea"] as const)(
    "has no rollback side effects when %s fails before the window can change",
    async (stage) => {
      const test = harness({ failForward: stage });

      await expect(new WindowSizeController(test.port).apply(1.25)).rejects.toBe(test.forwardError);

      expect(test.port.setRect).not.toHaveBeenCalled();
      expect(test.port.resizeRenderer).not.toHaveBeenCalled();
      expect(test.port.refreshHitRegion).not.toHaveBeenCalled();
    },
  );

  it("best-effort rolls back when setRect rejects because a compound native write may be partial", async () => {
    const test = harness({ failForward: "setRect" });

    await expect(new WindowSizeController(test.port).apply(1.25)).rejects.toBe(test.forwardError);

    expect(test.port.setRect).toHaveBeenCalledTimes(2);
    expect(test.port.setRect).toHaveBeenLastCalledWith(test.originalRect);
    expect(test.port.resizeRenderer).toHaveBeenCalledOnce();
    expect(test.port.refreshHitRegion).toHaveBeenCalledOnce();
  });

  it("normalizes a non-Error rejection before mutation without adding side effects", async () => {
    const test = harness({ failForward: "getRect", forwardError: "rect read failed" });

    let caught: WindowSizeApplyError | undefined;
    try {
      await new WindowSizeController(test.port).apply(1.25);
    } catch (error) {
      caught = error as WindowSizeApplyError;
    }

    expect(caught).toBeInstanceOf(Error);
    expect(caught?.message).toBe("rect read failed");
    expect(caught?.cause).toBe("rect read failed");
    expect(caught?.rollbackErrors).toEqual([]);
    expect(test.port.setRect).not.toHaveBeenCalled();
  });

  it.each(["resizeRenderer", "refreshHitRegion", "readback"] as const)(
    "rolls back the full visual pipeline when forward %s fails",
    async (stage) => {
      const test = harness({ failForward: stage });

      await expect(new WindowSizeController(test.port).apply(1.25)).rejects.toBe(test.forwardError);

      expect(test.port.setRect).toHaveBeenLastCalledWith(test.originalRect);
      expect(test.calls.slice(-3)).toEqual([
        "setRect:rollback",
        "resizeRenderer:rollback",
        "refreshHitRegion:rollback",
      ]);
    },
  );

  it("continues every rollback stage and attaches all rollback errors to the original failure", async () => {
    const setRectError = new Error("rollback setRect failed");
    const resizeError = new Error("rollback resize failed");
    const refreshError = new Error("rollback refresh failed");
    const test = harness({
      failForward: "refreshHitRegion",
      rollbackFailures: {
        setRect: setRectError,
        resizeRenderer: resizeError,
        refreshHitRegion: refreshError,
      },
    });

    let caught: WindowSizeApplyError | undefined;
    try {
      await new WindowSizeController(test.port).apply(1.25);
    } catch (error) {
      caught = error as WindowSizeApplyError;
    }

    expect(caught).toBe(test.forwardError);
    expect(caught?.cause).toEqual(new Error("native cause"));
    expect(caught?.rollbackErrors).toEqual([
      { stage: "setRect", error: setRectError },
      { stage: "resizeRenderer", error: resizeError },
      { stage: "refreshHitRegion", error: refreshError },
    ]);
    expect(test.calls.slice(-3)).toEqual([
      "setRect:rollback",
      "resizeRenderer:rollback",
      "refreshHitRegion:rollback",
    ]);
  });

  it.each(["setRect", "resizeRenderer", "refreshHitRegion"] as const)(
    "records an isolated %s rollback failure without skipping later compensation",
    async (rollbackStage) => {
      const rollbackError = new Error(`${rollbackStage} rollback failed`);
      const test = harness({
        failForward: "refreshHitRegion",
        rollbackFailures: { [rollbackStage]: rollbackError },
      });

      await new WindowSizeController(test.port).apply(1.25).catch((error: WindowSizeApplyError) => {
        expect(error).toBe(test.forwardError);
        expect(error.rollbackErrors).toEqual([{ stage: rollbackStage, error: rollbackError }]);
      });

      expect(test.calls.slice(-3)).toEqual([
        "setRect:rollback",
        "resizeRenderer:rollback",
        "refreshHitRegion:rollback",
      ]);
    },
  );

  it("wraps a non-Error failure when rollback errors must be attached", async () => {
    const rollbackSetError = "rollback set failed";
    const rollbackResizeError = new Error("rollback resize failed");
    const test = harness({
      failForward: "refreshHitRegion",
      forwardError: "refresh string failed",
      rollbackFailures: {
        setRect: rollbackSetError,
        resizeRenderer: rollbackResizeError,
      },
    });

    let caught: WindowSizeApplyError | undefined;
    try {
      await new WindowSizeController(test.port).apply(1.25);
    } catch (error) {
      caught = error as WindowSizeApplyError;
    }

    expect(caught).toBeInstanceOf(Error);
    expect(caught?.message).toBe("refresh string failed");
    expect(caught?.cause).toBe("refresh string failed");
    expect(caught?.rollbackErrors).toEqual([
      { stage: "setRect", error: rollbackSetError },
      { stage: "resizeRenderer", error: rollbackResizeError },
    ]);
  });

  it("wraps a frozen Error without hiding it behind an attachment TypeError", async () => {
    const nativeCause = new Error("native cause");
    const frozenError = Object.freeze(new Error("frozen refresh failed", { cause: nativeCause }));
    const rollbackErrors = {
      setRect: new Error("rollback set failed"),
      resizeRenderer: new Error("rollback resize failed"),
      refreshHitRegion: new Error("rollback refresh failed"),
    };
    const test = harness({
      failForward: "refreshHitRegion",
      forwardError: frozenError,
      rollbackFailures: rollbackErrors,
    });

    let caught: WindowSizeApplyError | undefined;
    try {
      await new WindowSizeController(test.port).apply(1.25);
    } catch (error) {
      caught = error as WindowSizeApplyError;
    }

    expect(caught).not.toBe(frozenError);
    expect(caught?.message).toBe("frozen refresh failed");
    expect(caught?.cause).toBe(frozenError);
    expect((caught?.cause as Error).cause).toBe(nativeCause);
    expect(caught?.rollbackErrors).toEqual([
      { stage: "setRect", error: rollbackErrors.setRect },
      { stage: "resizeRenderer", error: rollbackErrors.resizeRenderer },
      { stage: "refreshHitRegion", error: rollbackErrors.refreshHitRegion },
    ]);
  });

  it("waits for a failed transaction to finish rollback before starting the next request", async () => {
    const originalRect = { x: 1000, y: 400, width: 420, height: 520 };
    const workArea = { x: 0, y: 0, width: 1920, height: 1040 };
    const firstRefreshEntered = deferred();
    const releaseFirstRefresh = deferred();
    const firstError = new Error("first refresh failed");
    const getRectWidths: number[] = [];
    const setRectWidths: number[] = [];
    let currentRect = { ...originalRect };
    let refreshCalls = 0;
    const port: WindowSizePort = {
      getRect: vi.fn(async () => {
        getRectWidths.push(currentRect.width);
        return { ...currentRect };
      }),
      getWorkArea: vi.fn(async () => ({ ...workArea })),
      setRect: vi.fn(async (rect) => {
        setRectWidths.push(rect.width);
        currentRect = { ...rect };
      }),
      resizeRenderer: vi.fn(async () => undefined),
      refreshHitRegion: vi.fn(async () => {
        refreshCalls += 1;
        if (refreshCalls === 1) {
          firstRefreshEntered.resolve(undefined);
          await releaseFirstRefresh.promise;
          throw firstError;
        }
      }),
    };
    const controller = new WindowSizeController(port);

    const first = controller.apply(1.5);
    await firstRefreshEntered.promise;
    const firstRejected = first.catch((error: unknown) => error);
    const second = controller.apply(0.75);
    await Promise.resolve();

    expect(getRectWidths).toEqual([420]);
    releaseFirstRefresh.resolve(undefined);
    expect(await firstRejected).toBe(firstError);
    const secondAck = await second;

    expect(setRectWidths).toEqual([630, 420, 315]);
    expect(getRectWidths).toEqual([420, 420, 315]);
    expect(secondAck.rect).toEqual({ x: 1052, y: 530, width: 315, height: 390 });
    expect(currentRect).toEqual(secondAck.rect);
  });

  it("starts the next request from the previous transaction's actual acknowledged rectangle", async () => {
    const originalRect = { x: 1000, y: 400, width: 420, height: 520 };
    const actualFirstRect = { x: 900, y: 200, width: 600, height: 700 };
    const workArea = { x: 0, y: 0, width: 1920, height: 1040 };
    const firstRefreshEntered = deferred();
    const releaseFirstRefresh = deferred();
    const getRectWidths: number[] = [];
    let currentRect = { ...originalRect };
    let setRectCalls = 0;
    let refreshCalls = 0;
    const port: WindowSizePort = {
      getRect: vi.fn(async () => {
        getRectWidths.push(currentRect.width);
        return { ...currentRect };
      }),
      getWorkArea: vi.fn(async () => ({ ...workArea })),
      setRect: vi.fn(async (rect) => {
        setRectCalls += 1;
        currentRect = setRectCalls === 1 ? { ...actualFirstRect } : { ...rect };
      }),
      resizeRenderer: vi.fn(async () => undefined),
      refreshHitRegion: vi.fn(async () => {
        refreshCalls += 1;
        if (refreshCalls === 1) {
          firstRefreshEntered.resolve(undefined);
          await releaseFirstRefresh.promise;
        }
      }),
    };
    const controller = new WindowSizeController(port);

    const first = controller.apply(1.5);
    await firstRefreshEntered.promise;
    const second = controller.apply(0.5);
    await Promise.resolve();

    expect(getRectWidths).toEqual([420]);
    releaseFirstRefresh.resolve(undefined);
    const firstAck = await first;
    const secondAck = await second;

    expect(firstAck.rect).toEqual(actualFirstRect);
    expect(getRectWidths).toEqual([420, 600, 600, 210]);
    expect(secondAck.rect).toEqual({ x: 1095, y: 640, width: 210, height: 260 });
  });
});
