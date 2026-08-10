import { describe, expect, it, vi } from "vitest";
import type { PetRenderer } from "./pet-renderer";
import { PetStage } from "./pet-stage";

function fakeRenderer(): PetRenderer {
  return {
    load: vi.fn(async () => undefined),
    resize: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    setExpression: vi.fn(),
    setLookTarget: vi.fn(),
    setLipSync: vi.fn(),
    hitTest: vi.fn(() => "body" as const),
    setVisibility: vi.fn(),
    update: vi.fn(),
    destroy: vi.fn(),
  };
}

function fakeEventTarget() {
  const listeners = new Map<string, EventListener>();
  return {
    addEventListener: vi.fn((type: string, listener: EventListener) => listeners.set(type, listener)),
    removeEventListener: vi.fn((type: string) => listeners.delete(type)),
    emit(type: string, event: object) { listeners.get(type)?.(event as Event); },
  };
}

function harness() {
  const renderer = fakeRenderer();
  const rootEvents = fakeEventTarget();
  const pointerEvents = fakeEventTarget();
  const resizeEvents = fakeEventTarget();
  const root = {
    ...rootEvents,
    clientWidth: 420,
    clientHeight: 520,
    getBoundingClientRect: () => ({ left: 10, top: 20 }),
  } as unknown as HTMLElement & { emit(type: string, event: object): void };
  let frame: FrameRequestCallback | undefined;
  const windowMotion = {
    beginDrag: vi.fn(async () => undefined),
    dragTo: vi.fn(async () => undefined),
    endDrag: vi.fn(async () => undefined),
    update: vi.fn(async () => undefined),
    shake: vi.fn(),
    bounce: vi.fn(),
  };
  const effects = { play: vi.fn(), destroy: vi.fn() };
  const refreshHitRegion = vi.fn(async () => undefined);
  const stage = new PetStage({
    renderer,
    windowMotion,
    effects,
    pointerTarget: pointerEvents,
    resizeTarget: resizeEvents,
    requestFrame: (callback) => { frame = callback; return 7; },
    cancelFrame: vi.fn(),
    devicePixelRatio: () => 2,
    setTimer: vi.fn(() => 1),
    clearTimer: vi.fn(),
    refreshHitRegion,
  });
  return {
    effects,
    frame: () => frame,
    pointerEvents,
    refreshHitRegion,
    renderer,
    resizeEvents,
    root,
    stage,
    windowMotion,
  };
}

describe("PetStage", () => {
  it("mounts through the renderer contract and starts the idle loop", async () => {
    const { renderer, root, stage } = harness();

    await stage.mount(root);

    expect(renderer.resize).toHaveBeenCalledWith({ width: 420, height: 520, dpr: 2 });
    expect(renderer.setVisibility).toHaveBeenCalledWith(true);
    expect(renderer.playMotion).toHaveBeenCalledWith("idle", { priority: 10, loop: true });
  });

  it("routes a body click through behavior and presentation", async () => {
    const { pointerEvents, renderer, root, stage } = harness();
    await stage.mount(root);

    root.emit("pointerdown", { button: 0, clientX: 110, clientY: 220 });
    pointerEvents.emit("pointerup", { clientX: 110, clientY: 220 });
    await Promise.resolve();

    expect(renderer.hitTest).toHaveBeenCalledWith({ x: 100, y: 200 });
    expect(renderer.setExpression).toHaveBeenCalledWith("curious");
    expect(renderer.playMotion).toHaveBeenCalledWith("react-curious", { priority: 60 });
  });

  it("uses screen coordinates for desktop window dragging", async () => {
    const { pointerEvents, root, stage, windowMotion } = harness();
    await stage.mount(root);

    root.emit("pointerdown", { button: 0, clientX: 20, clientY: 30, screenX: 120, screenY: 230 });
    pointerEvents.emit("pointermove", { clientX: 21, clientY: 31, screenX: 130, screenY: 245 });
    await vi.waitFor(() => expect(windowMotion.dragTo).toHaveBeenCalled());

    expect(windowMotion.beginDrag).toHaveBeenCalledWith({ x: 120, y: 230 }, 2);
    expect(windowMotion.dragTo).toHaveBeenCalledWith({ x: 130, y: 245 });
  });

  it("advances renderer and transient window motion from one frame clock", async () => {
    const { frame, renderer, root, stage, windowMotion } = harness();
    await stage.mount(root);

    frame()?.(16);
    await Promise.resolve();

    expect(renderer.update).toHaveBeenCalledWith(16);
    expect(windowMotion.update).toHaveBeenCalledWith(16);
  });

  it("refreshes the transparent window hit region after the initial render", async () => {
    const { refreshHitRegion, root, stage } = harness();

    await stage.mount(root);

    expect(refreshHitRegion).toHaveBeenCalledOnce();
  });

  it("refreshes the transparent hit region after a resize", async () => {
    const { refreshHitRegion, resizeEvents, root, stage } = harness();
    await stage.mount(root);

    resizeEvents.emit("resize", {});
    await vi.waitFor(() => expect(refreshHitRegion).toHaveBeenCalledTimes(2));
  });

  it("pauses while hidden and restores the awake presentation", async () => {
    const { renderer, root, stage } = harness();
    await stage.mount(root);

    stage.setVisibility(false);
    stage.setVisibility(true);

    expect(renderer.setVisibility).toHaveBeenNthCalledWith(2, false);
    expect(renderer.setVisibility).toHaveBeenNthCalledWith(3, true);
    expect(renderer.playMotion).toHaveBeenCalledWith("sleep", { priority: 50, loop: true });
    expect(renderer.playMotion).toHaveBeenCalledWith("wake", { priority: 50 });
  });

  it("destroys owned runtime ports idempotently", async () => {
    const { effects, renderer, root, stage } = harness();
    await stage.mount(root);

    stage.destroy();
    stage.destroy();

    expect(renderer.destroy).toHaveBeenCalledOnce();
    expect(effects.destroy).toHaveBeenCalledOnce();
  });
});
