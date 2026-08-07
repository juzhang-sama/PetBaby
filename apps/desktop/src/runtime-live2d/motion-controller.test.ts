import { describe, expect, it, vi } from "vitest";
import { MotionController, type MotionPlaybackPort } from "./motion-controller";

function fakePort() {
  let finish: (() => void) | undefined;
  const cancel = vi.fn();
  const port: MotionPlaybackPort = {
    start: vi.fn((_name, _options, onFinished) => {
      finish = onFinished;
      return { cancel };
    }),
    stopAll: vi.fn(),
  };
  return { port, cancel, finish: () => finish?.() };
}

describe("MotionController", () => {
  it("does not let idle interrupt carried", () => {
    const { port, cancel } = fakePort();
    const controller = new MotionController({ port, now: () => 100 });

    controller.play("carried", { priority: 80, loop: true });
    const rejected = controller.play("idle", { priority: 10, loop: true });

    expect(controller.current()?.name).toBe("carried");
    expect(port.start).toHaveBeenCalledTimes(1);
    rejected.cancel();
    expect(cancel).not.toHaveBeenCalled();
  });

  it("replaces an equal-priority motion and cancels the old playback once", () => {
    const { port, cancel } = fakePort();
    const controller = new MotionController({ port });

    controller.play("react-happy", { priority: 60 });
    controller.play("react-curious", { priority: 60 });

    expect(controller.current()?.name).toBe("react-curious");
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it("resumes the current state loop after a one-shot motion finishes", () => {
    const fake = fakePort();
    const controller = new MotionController({
      port: fake.port,
      resumeForState: () => ({ name: "sleep", priority: 50, loop: true }),
    });
    controller.play("landed", { priority: 80, loop: false });

    fake.finish();

    expect(controller.current()).toMatchObject({ name: "sleep", priority: 50, loop: true });
    expect(fake.port.start).toHaveBeenLastCalledWith(
      "sleep",
      { priority: 50, loop: true },
      expect.any(Function),
    );
  });

  it("stops the active motion idempotently", () => {
    const { port, cancel } = fakePort();
    const controller = new MotionController({ port });
    controller.play("idle", { priority: 10, loop: true });

    controller.stopAll();
    controller.stopAll();

    expect(cancel).toHaveBeenCalledTimes(1);
    expect(port.stopAll).toHaveBeenCalledTimes(1);
    expect(controller.current()).toBeNull();
  });
});
