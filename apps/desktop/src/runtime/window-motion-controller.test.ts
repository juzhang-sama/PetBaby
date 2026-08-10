import { describe, expect, it, vi } from "vitest";
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
});
