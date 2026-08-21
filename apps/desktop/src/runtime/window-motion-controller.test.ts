import { describe, expect, it, vi } from "vitest";
import * as windowMotionModule from "./window-motion-controller";
import { WindowMotionController, type WindowMotionPort } from "./window-motion-controller";

function fakePort(position = { x: 100, y: 200 }) {
  let current = { ...position };
  const savedPositions: Array<{ x: number; y: number }> = [];
  const port: WindowMotionPort = {
    getPosition: vi.fn(async () => ({ ...current })),
    setPosition: vi.fn(async (next) => { current = { ...next }; }),
    persistPosition: vi.fn(async (next) => { savedPositions.push({ ...next }); }),
  };
  return { port, current: () => current, savedPositions };
}

describe("WindowMotionController", () => {
  it("does not persist transient shake offsets", async () => {
    const fake = fakePort();
    const controller = new WindowMotionController(fake.port);

    controller.shake({ amplitude: 4, durationMs: 120 });
    await controller.update(60);

    expect(fake.port.setPosition).toHaveBeenCalled();
    expect(fake.savedPositions).toEqual([]);
  });

  it("restores the baseline when a shake finishes", async () => {
    const fake = fakePort();
    const controller = new WindowMotionController(fake.port);
    controller.shake({ amplitude: 4, durationMs: 120 });

    await controller.update(60);
    await controller.update(60);

    expect(fake.current()).toEqual({ x: 100, y: 200 });
    expect(fake.savedPositions).toEqual([]);
  });

  it("persists a drag only when the drag ends", async () => {
    const fake = fakePort();
    const controller = new WindowMotionController(fake.port);

    await controller.beginDrag({ x: 10, y: 20 }, 2);
    await controller.dragTo({ x: 20, y: 35 });
    expect(fake.savedPositions).toEqual([]);

    await controller.endDrag();

    expect(fake.current()).toEqual({ x: 120, y: 230 });
    expect(fake.savedPositions).toEqual([{ x: 120, y: 230 }]);
  });

  it("freezes synchronously, waits for in-flight motion, cancels the drag, and resumes cleanly", async () => {
    let current = { x: 100, y: 200 };
    let releaseMove!: () => void;
    const moveBlocked = new Promise<void>((resolve) => { releaseMove = resolve; });
    let blockFirstMove = true;
    const port: WindowMotionPort = {
      getPosition: vi.fn(async () => ({ ...current })),
      setPosition: vi.fn(async (next) => {
        if (blockFirstMove) {
          blockFirstMove = false;
          await moveBlocked;
        }
        current = { ...next };
      }),
      persistPosition: vi.fn(async () => undefined),
    };
    const controller = new WindowMotionController(port);
    const suspend = (controller as unknown as {
      suspend?: () => Promise<{ release(): void }>;
    }).suspend;
    expect(suspend).toEqual(expect.any(Function));
    if (!suspend) return;

    await controller.beginDrag({ x: 0, y: 0 });
    const inFlightMove = controller.dragTo({ x: 10, y: 20 });
    const suspensionPromise = suspend.call(controller);
    const ignoredMove = controller.dragTo({ x: 50, y: 60 });
    let suspensionSettled = false;
    void suspensionPromise.then(() => { suspensionSettled = true; });
    await Promise.resolve();

    expect(port.setPosition).toHaveBeenCalledOnce();
    expect(suspensionSettled).toBe(false);
    releaseMove();
    await inFlightMove;
    const suspension = await suspensionPromise;
    await ignoredMove;
    await controller.endDrag();
    expect(port.persistPosition).not.toHaveBeenCalled();

    suspension.release();
    await controller.beginDrag({ x: 50, y: 60 });
    await controller.dragTo({ x: 55, y: 65 });
    await controller.endDrag();

    expect(current).toEqual({ x: 115, y: 225 });
    expect(port.persistPosition).toHaveBeenCalledWith({ x: 115, y: 225 });
  });

  it("releases motion after a coordinated flush or scale failure", async () => {
    const coordinate = (windowMotionModule as unknown as {
      runWithWindowMotionSuspended?: <T>(
        motion: WindowMotionController,
        flush: () => Promise<void>,
        operation: () => Promise<T>,
      ) => Promise<T>;
    }).runWithWindowMotionSuspended;
    expect(coordinate).toEqual(expect.any(Function));
    if (!coordinate) return;
    const errors = [new Error("flush failed"), new Error("scale failed")];

    for (const [index, error] of errors.entries()) {
      const fake = fakePort();
      const controller = new WindowMotionController(fake.port);
      const flush = index === 0
        ? vi.fn(async () => { throw error; })
        : vi.fn(async () => undefined);
      const operation = index === 1
        ? vi.fn(async () => { throw error; })
        : vi.fn(async () => undefined);

      await expect(coordinate(controller, flush, operation)).rejects.toBe(error);
      await controller.beginDrag({ x: 0, y: 0 });
      await controller.dragTo({ x: 5, y: 5 });

      expect(fake.port.setPosition).toHaveBeenCalledWith({ x: 105, y: 205 });
      if (index === 0) expect(operation).not.toHaveBeenCalled();
    }
  });
});
