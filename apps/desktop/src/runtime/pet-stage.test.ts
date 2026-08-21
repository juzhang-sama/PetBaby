import { describe, expect, it, vi } from "vitest";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import * as bridgeModule from "./bridge";
import * as contractsModule from "./contracts";
import { DEFAULT_PET_CALIBRATION, type PetCalibrationV1 } from "./pet-calibration";
import type { PetRenderer } from "./pet-renderer";
import type { PetEffect, PetEffectVisualOptions } from "./pet-presentation-controller";
import { PetStage, type StageEffectOverlay } from "./pet-stage";
import * as petStageModule from "./pet-stage";
import * as windowMotionModule from "./window-motion-controller";
import { WindowMotionController } from "./window-motion-controller";
import {
  WindowSizeController,
  type WindowSizeAck,
  type WindowSizePort,
} from "./window-size-controller";

type RuntimeExports = Record<string, unknown>;

const bridge = bridgeModule as unknown as RuntimeExports;
const contracts = contractsModule as unknown as RuntimeExports;

function fakeRenderer(): PetRenderer {
  return {
    load: vi.fn(async () => undefined),
    resize: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    setExpression: vi.fn(),
    setLookTarget: vi.fn(),
    setLipSync: vi.fn(),
    setCalibration: vi.fn(),
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

function harness(options: { catV4?: boolean; onFrameSample?: (deltas: readonly number[]) => void } = {}) {
  const renderer = fakeRenderer();
  if (options.catV4) {
    Object.assign(renderer, {
      supportsCatMotionV1: vi.fn(() => true),
      playCatMotion: vi.fn(() => ({ cancel: vi.fn() })),
      setCatAutomationMode: vi.fn(),
    });
  }
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
  const effects: StageEffectOverlay = {
    play: vi.fn((_effect: PetEffect, _options: PetEffectVisualOptions) => undefined),
    destroy: vi.fn(),
  };
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
    onFrameSample: options.onFrameSample,
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
  it("does not expose or schedule blink while the feature is frozen", async () => {
    const { frame, renderer, root, stage } = harness();

    expect("setBlink" in renderer).toBe(false);
    await stage.mount(root);
    frame()?.(1_000);
    frame()?.(50_000);
  });

  it("synchronizes calibration without scheduling the frozen blink feature", async () => {
    const { frame, renderer, root, stage } = harness();
    const value: PetCalibrationV1 = {
      schemaVersion: 1,
      breathAmplitudePercent: 4,
      blinkIntervalScale: 0.5,
      feedbackStrength: 0.8,
    };

    stage.setCalibration(value);
    await stage.mount(root);

    frame()?.(1_000);
    frame()?.(2_500);
    expect(renderer.setCalibration).toHaveBeenCalledWith(value);
    stage.setVisibility(false);
    stage.setVisibility(true);
    frame()?.(50_000);
    expect("setBlink" in renderer).toBe(false);
  });

  it("previews curious feedback without running behavior selection", async () => {
    const { effects, renderer, root, stage } = harness();
    stage.setCalibration({ ...DEFAULT_PET_CALIBRATION, feedbackStrength: 0.25 });
    await stage.mount(root);
    vi.mocked(renderer.setExpression).mockClear();
    vi.mocked(renderer.playMotion).mockClear();

    stage.previewFeedback();

    expect(renderer.setExpression).toHaveBeenCalledWith("curious");
    expect(renderer.playMotion).toHaveBeenCalledWith("react-curious", { priority: 60 });
    expect(effects.play).toHaveBeenCalledWith("sparkles", { opacity: 0.25, intensity: 0.25 });
  });

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

  it("routes v4 mount, body petting and completion through the cat motion scheduler", async () => {
    const { pointerEvents, renderer, root, stage } = harness({ catV4: true });
    const playCatMotion = vi.mocked(renderer.playCatMotion!);
    let complete: (() => void) | undefined;
    playCatMotion.mockImplementation((_motion, _options, onFinished) => {
      complete = onFinished;
      return { cancel: vi.fn() };
    });

    await stage.mount(root);
    expect(playCatMotion).toHaveBeenCalledWith(
      "breathing",
      expect.objectContaining({ priority: 10, loop: true, fadeInMs: 180, fadeOutMs: 140 }),
      expect.any(Function),
    );
    playCatMotion.mockClear();

    root.emit("pointerdown", { button: 0, clientX: 110, clientY: 220, screenX: 110, screenY: 220 });
    pointerEvents.emit("pointerup", { clientX: 110, clientY: 220 });
    await Promise.resolve();

    expect(playCatMotion).toHaveBeenCalledWith(
      "pet-happy",
      expect.objectContaining({ priority: 90, loop: false }),
      expect.any(Function),
    );
    expect(renderer.setLookTarget).toHaveBeenCalledWith(null);
    complete?.();
    expect(playCatMotion).toHaveBeenLastCalledWith(
      "breathing",
      expect.objectContaining({ priority: 10, loop: true }),
      expect.any(Function),
    );
  });

  it("cancels the current v4 cat action once movement crosses the drag threshold", async () => {
    const { pointerEvents, renderer, root, stage, windowMotion } = harness({ catV4: true });
    const cancel = vi.fn();
    const playCatMotion = vi.mocked(renderer.playCatMotion!);
    playCatMotion.mockReturnValue({ cancel });
    await stage.mount(root);
    cancel.mockClear();

    root.emit("pointerdown", { button: 0, clientX: 10, clientY: 10, screenX: 10, screenY: 10 });
    pointerEvents.emit("pointermove", { clientX: 20, clientY: 20, screenX: 20, screenY: 20 });
    await vi.waitFor(() => expect(windowMotion.dragTo).toHaveBeenCalled());

    expect(cancel).toHaveBeenCalled();
    expect(playCatMotion).not.toHaveBeenCalledWith(
      "pet-happy",
      expect.anything(),
      expect.anything(),
    );
  });

  it("routes v4 pointer enter and leave through focus and idle motions", async () => {
    const { renderer, root, stage } = harness({ catV4: true });
    const playCatMotion = vi.mocked(renderer.playCatMotion!);
    await stage.mount(root);
    playCatMotion.mockClear();
    vi.mocked(renderer.setCatAutomationMode!).mockClear();

    root.emit("pointerenter", {});
    root.emit("pointerleave", {});

    expect(playCatMotion).toHaveBeenNthCalledWith(
      1,
      "pointer-focus",
      expect.objectContaining({ priority: 60, loop: true }),
      expect.any(Function),
    );
    expect(playCatMotion).toHaveBeenNthCalledWith(
      2,
      "breathing",
      expect.objectContaining({ priority: 10, loop: true }),
      expect.any(Function),
    );
    expect(renderer.setCatAutomationMode).toHaveBeenNthCalledWith(1, "pointerFocus");
    expect(renderer.setCatAutomationMode).toHaveBeenLastCalledWith("idle");
  });

  it("uses 60 fps for v4 companion and suppresses automation while dragging", async () => {
    const { frame, pointerEvents, renderer, root, stage, windowMotion } = harness({ catV4: true });
    await stage.mount(root);
    vi.mocked(renderer.update).mockClear();

    frame()?.(1_000);
    frame()?.(1_017);
    expect(renderer.update).toHaveBeenLastCalledWith(17);

    root.emit("pointerdown", { button: 0, clientX: 10, clientY: 10, screenX: 10, screenY: 10 });
    pointerEvents.emit("pointermove", { clientX: 20, clientY: 20, screenX: 20, screenY: 20 });
    await vi.waitFor(() => expect(windowMotion.dragTo).toHaveBeenCalled());
    expect(renderer.setCatAutomationMode).toHaveBeenLastCalledWith("dragging");
    expect(renderer.setLookTarget).toHaveBeenCalledWith(null);
  });

  it("emits bounded one-second frame samples only for rendered frames", async () => {
    const onFrameSample = vi.fn();
    const { frame, root, stage } = harness({ catV4: true, onFrameSample });
    await stage.mount(root);

    for (let now = 1_000; now <= 2_100; now += 17) frame()?.(now);

    expect(onFrameSample).toHaveBeenCalled();
    const deltas = onFrameSample.mock.calls[0]![0] as readonly number[];
    expect(deltas.length).toBeGreaterThan(50);
    expect(deltas.every((delta) => delta > 0 && delta < 100)).toBe(true);
  });

  it.each([
    [true, 59, 61],
    [false, 23, 25],
  ])("renders the %s v4 capability at its real companion cadence", async (catV4, minFrames, maxFrames) => {
    const { frame, renderer, root, stage } = harness({ catV4 });
    await stage.mount(root);
    vi.mocked(renderer.update).mockClear();

    for (let index = 0; index <= 60; index += 1) frame()?.(1_000 + index * (1_000 / 60));

    expect(vi.mocked(renderer.update).mock.calls.length).toBeGreaterThanOrEqual(minFrames);
    expect(vi.mocked(renderer.update).mock.calls.length).toBeLessThanOrEqual(maxFrames);
  });

  it("updates only the v4 look target during pointer movement without starting a drag", async () => {
    const { pointerEvents, renderer, root, stage, windowMotion } = harness({ catV4: true });
    await stage.mount(root);

    pointerEvents.emit("pointermove", { clientX: 325, clientY: 150, screenX: 900, screenY: 700 });

    expect(renderer.setLookTarget).toHaveBeenCalledWith({ x: 0.5, y: 0.5 });
    expect(windowMotion.beginDrag).not.toHaveBeenCalled();
    expect(windowMotion.dragTo).not.toHaveBeenCalled();
  });

  it("restarts v4 scheduling when a hot switch replaces one v4 renderer with another", async () => {
    const { renderer, root, stage } = harness({ catV4: true });
    const playCatMotion = vi.mocked(renderer.playCatMotion!);
    await stage.mount(root);
    playCatMotion.mockClear();

    stage.syncActiveRenderer();

    expect(playCatMotion).toHaveBeenCalledWith(
      "breathing",
      expect.objectContaining({ priority: 10, loop: true }),
      expect.any(Function),
    );
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

  it("starts and restarts the frame clock at zero without hidden catch-up", async () => {
    const { frame, renderer, root, stage, windowMotion } = harness();
    await stage.mount(root);
    vi.mocked(renderer.update).mockClear();
    windowMotion.update.mockClear();

    frame()?.(16_000);
    await Promise.resolve();
    expect(renderer.update).toHaveBeenLastCalledWith(0);
    expect(windowMotion.update).toHaveBeenLastCalledWith(0);

    frame()?.(16_050);
    await Promise.resolve();
    expect(renderer.update).toHaveBeenLastCalledWith(50);
    expect(windowMotion.update).toHaveBeenLastCalledWith(50);

    stage.setVisibility(false);
    stage.setVisibility(true);
    frame()?.(60_000);
    await Promise.resolve();

    expect(renderer.update).toHaveBeenLastCalledWith(0);
    expect(windowMotion.update).toHaveBeenLastCalledWith(0);
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

  it("exposes a viewport refresh that resizes the renderer from the current root", async () => {
    const { renderer, root, stage } = harness();
    await stage.mount(root);
    vi.mocked(renderer.resize).mockClear();
    Object.defineProperty(root, "clientWidth", { configurable: true, value: 315 });
    Object.defineProperty(root, "clientHeight", { configurable: true, value: 390 });

    await (stage as PetStage & { refreshViewport(): Promise<void> }).refreshViewport();

    expect(renderer.resize).toHaveBeenCalledWith({ width: 315, height: 390, dpr: 2 });
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

  it("freezes a host transition and resumes from the native effective visibility fact", async () => {
    const { frame, renderer, root, stage, windowMotion } = harness();
    await stage.mount(root);
    vi.mocked(renderer.update).mockClear();
    windowMotion.update.mockClear();

    stage.pauseWindowModeTransition();
    frame()?.(20_000);
    await Promise.resolve();
    expect(renderer.setVisibility).toHaveBeenLastCalledWith(false);
    expect(renderer.update).not.toHaveBeenCalled();
    expect(windowMotion.update).not.toHaveBeenCalled();

    stage.resumeWindowModeTransition(false);
    frame()?.(40_000);
    await Promise.resolve();
    expect(renderer.setVisibility).toHaveBeenLastCalledWith(false);
    expect(renderer.update).not.toHaveBeenCalled();
    expect(windowMotion.update).not.toHaveBeenCalled();

    stage.pauseWindowModeTransition();
    stage.resumeWindowModeTransition(true);
    frame()?.(60_000);
    expect(renderer.setVisibility).toHaveBeenLastCalledWith(true);
    expect(renderer.update).toHaveBeenLastCalledWith(0);
  });

  it("aborts a rejected runtime ACK into a hidden paused state that a later cycle can recover", async () => {
    const { frame, renderer, root, stage } = harness();
    await stage.mount(root);
    vi.mocked(renderer.update).mockClear();
    stage.pauseWindowModeTransition();
    stage.abortWindowModeTransition();
    frame()?.(20_000);
    expect(renderer.setVisibility).toHaveBeenLastCalledWith(false);
    expect(renderer.update).not.toHaveBeenCalled();
    stage.pauseWindowModeTransition();
    stage.resumeWindowModeTransition(true);
    frame()?.(40_000);
    expect(renderer.setVisibility).toHaveBeenLastCalledWith(true);
  });
});

describe("pet calibration preview protocol", () => {
  const calibration = (overrides: Partial<PetCalibrationV1> = {}): PetCalibrationV1 => ({
    ...DEFAULT_PET_CALIBRATION,
    ...overrides,
  });

  it("accepts only exact canonical requests and exact result variants", () => {
    const requestGuard = contracts.isPetCalibrationPreviewRequest as ((value: unknown) => boolean) | undefined;
    const resultGuard = contracts.isPetCalibrationPreviewResult as ((value: unknown) => boolean) | undefined;
    const request = {
      requestId: "calibration-1",
      petId: "pet_a-1",
      action: "preview",
      value: calibration({ feedbackStrength: 1 }),
    };
    expect(requestGuard).toEqual(expect.any(Function));
    expect(resultGuard).toEqual(expect.any(Function));
    if (!requestGuard || !resultGuard) return;

    expect(requestGuard(request)).toBe(true);
    for (const invalid of [
      { ...request, requestId: "bad id" },
      { ...request, petId: "../pet" },
      { ...request, action: "save" },
      { ...request, value: { ...request.value, feedbackStrength: Number.NaN } },
      { ...request, value: { ...request.value, blinkIntervalScale: 2.1 } },
      { ...request, extra: true },
    ]) expect(requestGuard(invalid)).toBe(false);

    expect(resultGuard({ ...request, ok: true })).toBe(true);
    expect(resultGuard({
      requestId: request.requestId,
      petId: request.petId,
      action: request.action,
      ok: false,
      message: "inactive pet",
    })).toBe(true);
    expect(resultGuard({ ...request, ok: true, extra: true })).toBe(false);
  });

  it("listens before emitting and ignores late, wrong-pet, and wrong-action results", async () => {
    const request = petStageModule.requestPetCalibrationPreview as unknown as ((
      petId: string,
      action: "preview" | "restore" | "feedback",
      value: PetCalibrationV1,
      options: Record<string, unknown>,
    ) => Promise<unknown>) | undefined;
    expect(request).toEqual(expect.any(Function));
    if (!request) return;
    const events: string[] = [];
    let receive!: (value: unknown) => void;
    const dispose = vi.fn(() => events.push("dispose"));
    const promise = request("pet-a", "preview", calibration({ feedbackStrength: 1 }), {
      requestIdFactory: () => "request-1",
      timeoutMs: 100,
      ports: {
        listen: async (handler: (value: unknown) => void) => {
          events.push("listen");
          receive = handler;
          return dispose;
        },
        emit: async () => { events.push("emit"); },
      },
    });
    await vi.waitFor(() => expect(events).toEqual(["listen", "emit"]));
    receive({ requestId: "request-1", petId: "pet-b", action: "preview", ok: true, value: calibration() });
    receive({ requestId: "request-1", petId: "pet-a", action: "restore", ok: true, value: calibration() });
    receive({ requestId: "request-1", petId: "pet-a", action: "preview", ok: true, value: calibration({ feedbackStrength: 1 }) });
    await expect(promise).resolves.toMatchObject({ ok: true, petId: "pet-a", action: "preview" });
    expect(dispose).toHaveBeenCalledOnce();
    receive({ requestId: "request-1", petId: "pet-a", action: "preview", ok: true, value: calibration() });
    expect(dispose).toHaveBeenCalledOnce();
  });

  it("times out with cleanup and rejects emit errors", async () => {
    const request = petStageModule.requestPetCalibrationPreview as unknown as ((
      petId: string,
      action: "preview" | "restore" | "feedback",
      value: PetCalibrationV1,
      options: Record<string, unknown>,
    ) => Promise<unknown>) | undefined;
    expect(request).toEqual(expect.any(Function));
    if (!request) return;
    vi.useFakeTimers();
    const dispose = vi.fn();
    const timeout = request("pet-a", "restore", calibration(), {
      requestIdFactory: () => "request-timeout",
      timeoutMs: 10,
      ports: { listen: async () => dispose, emit: async () => undefined },
    });
    const timeoutExpectation = expect(timeout).rejects.toThrow(/timed out/i);
    await vi.advanceTimersByTimeAsync(10);
    await timeoutExpectation;
    expect(dispose).toHaveBeenCalledOnce();
    vi.useRealTimers();

    await expect(request("pet-a", "feedback", calibration(), {
      requestIdFactory: () => "request-error",
      ports: { listen: async () => vi.fn(), emit: async () => { throw new Error("emit failed"); } },
    })).rejects.toThrow("emit failed");
  });

  it("previews then restores saved state and rejects requests for inactive pets", async () => {
    const register = petStageModule.listenForPetCalibrationPreviewRequests as unknown as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    expect(register).toEqual(expect.any(Function));
    if (!register) return;
    let receive!: (value: unknown) => void;
    const applied: PetCalibrationV1[] = [];
    const emitted: unknown[] = [];
    const feedback = vi.fn();
    const saved = calibration({ feedbackStrength: 0.6 });
    await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return vi.fn(); },
      emit: async (value: unknown) => { emitted.push(value); },
      activePetId: () => "pet-a",
      savedCalibration: () => saved,
      setCalibration: (value: PetCalibrationV1) => { applied.push(value); },
      commitSaved: vi.fn(),
      previewFeedback: feedback,
    });

    receive({ requestId: "preview-1", petId: "pet-a", action: "preview", value: calibration({ feedbackStrength: 1 }) });
    receive({ requestId: "restore-1", petId: "pet-a", action: "restore", value: calibration() });
    receive({ requestId: "feedback-1", petId: "pet-a", action: "feedback", value: calibration({ feedbackStrength: 0.2 }) });
    receive({ requestId: "stale-1", petId: "pet-b", action: "preview", value: calibration({ feedbackStrength: 0.9 }) });
    await vi.waitFor(() => expect(emitted).toHaveLength(4));

    expect(applied.map((value) => value.feedbackStrength)).toEqual([1, 0.6, 0.2]);
    expect(feedback).toHaveBeenCalledOnce();
    expect(emitted.at(-1)).toMatchObject({ requestId: "stale-1", petId: "pet-b", ok: false });
  });

  it("binds an in-flight request id once and rejects conflicting reuse without another mutation", async () => {
    const register = petStageModule.listenForPetCalibrationPreviewRequests as unknown as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    expect(register).toEqual(expect.any(Function));
    if (!register) return;
    let receive!: (value: unknown) => void;
    const emitted: unknown[] = [];
    const setCalibration = vi.fn();
    await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return vi.fn(); },
      emit: async (value: unknown) => { emitted.push(value); },
      activePetId: () => "pet-a",
      savedCalibration: () => calibration(),
      setCalibration,
      commitSaved: vi.fn(),
      previewFeedback: vi.fn(),
    });

    receive({ requestId: "same-id", petId: "pet-a", action: "preview", value: calibration({ feedbackStrength: 0.8 }) });
    receive({ requestId: "same-id", petId: "pet-a", action: "preview", value: calibration({ feedbackStrength: 0.2 }) });
    await vi.waitFor(() => expect(emitted).toHaveLength(2));

    expect(setCalibration).toHaveBeenCalledOnce();
    expect(emitted[1]).toMatchObject({ requestId: "same-id", ok: false });
  });

  it("commits a canonical preview as the new saved restore point", async () => {
    const Runtime = petStageModule.PetCalibrationRuntime as unknown as (new (options: Record<string, unknown>) => {
      activate(petId: string): Promise<void>;
      savedCalibration(): PetCalibrationV1;
      commitSaved(petId: string, value: PetCalibrationV1): void;
    }) | undefined;
    const register = petStageModule.listenForPetCalibrationPreviewRequests as unknown as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    expect(Runtime).toEqual(expect.any(Function));
    expect(register).toEqual(expect.any(Function));
    if (!Runtime || !register) return;
    const applied: PetCalibrationV1[] = [];
    const runtime = new Runtime({
      activePetId: () => "pet-a",
      load: async () => calibration({ feedbackStrength: 0.6 }),
      setCalibration: (value: PetCalibrationV1) => { applied.push(value); },
    });
    await runtime.activate("pet-a");
    let receive!: (value: unknown) => void;
    const emitted: unknown[] = [];
    await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return vi.fn(); },
      emit: async (value: unknown) => { emitted.push(value); },
      activePetId: () => "pet-a",
      savedCalibration: () => runtime.savedCalibration(),
      setCalibration: (value: PetCalibrationV1) => { applied.push(value); },
      commitSaved: (petId: string, value: PetCalibrationV1) => runtime.commitSaved(petId, value),
      previewFeedback: vi.fn(),
    });
    const send = async (requestId: string, action: string, feedbackStrength: number): Promise<void> => {
      receive({ requestId, petId: "pet-a", action, value: calibration({ feedbackStrength }) });
      await vi.waitFor(() => expect(emitted).toHaveLength(Number(requestId.split("-").at(-1))));
    };

    await send("step-1", "preview", 1);
    await send("step-2", "commit", 1);
    await send("step-3", "preview", 0.2);
    await send("step-4", "restore", 0.2);

    expect(runtime.savedCalibration().feedbackStrength).toBe(1);
    expect(applied.at(-1)?.feedbackStrength).toBe(1);
  });

  it("rejects wrong-pet and invalid saved commits without polluting the saved snapshot", async () => {
    const Runtime = petStageModule.PetCalibrationRuntime as unknown as (new (options: Record<string, unknown>) => {
      activate(petId: string): Promise<void>;
      savedCalibration(): PetCalibrationV1;
      commitSaved(petId: string, value: PetCalibrationV1): void;
    }) | undefined;
    expect(Runtime).toEqual(expect.any(Function));
    if (!Runtime) return;
    const setCalibration = vi.fn();
    const runtime = new Runtime({
      activePetId: () => "pet-a",
      load: async () => calibration({ feedbackStrength: 0.6 }),
      setCalibration,
    });
    await runtime.activate("pet-a");
    const before = runtime.savedCalibration();
    const callsBeforeInvalidCommits = setCalibration.mock.calls.length;

    expect(() => runtime.commitSaved("pet-b", calibration({ feedbackStrength: 1 }))).toThrow(/active pet/i);
    expect(() => runtime.commitSaved("pet-a", { ...calibration(), feedbackStrength: Number.NaN })).toThrow(/finite/i);
    expect(runtime.savedCalibration()).toEqual(before);
    expect(setCalibration).toHaveBeenCalledTimes(callsBeforeInvalidCommits);
  });

  it("stops queued and future preview work after listener teardown", async () => {
    const register = petStageModule.listenForPetCalibrationPreviewRequests as unknown as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    expect(register).toEqual(expect.any(Function));
    if (!register) return;
    let receive!: (value: unknown) => void;
    const unlisten = vi.fn();
    const emit = vi.fn();
    const setCalibration = vi.fn();
    const destroy = await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return unlisten; },
      emit,
      activePetId: () => "pet-a",
      savedCalibration: () => calibration(),
      setCalibration,
      commitSaved: vi.fn(),
      previewFeedback: vi.fn(),
    });
    receive({ requestId: "queued", petId: "pet-a", action: "preview", value: calibration({ feedbackStrength: 1 }) });
    destroy();
    receive({ requestId: "future", petId: "pet-a", action: "preview", value: calibration({ feedbackStrength: 0.2 }) });
    await Promise.resolve();
    await Promise.resolve();

    expect(unlisten).toHaveBeenCalledOnce();
    expect(setCalibration).not.toHaveBeenCalled();
    expect(emit).not.toHaveBeenCalled();
  });

  it("loads saved calibration for the active pet and discards stale switch loads", async () => {
    const Runtime = petStageModule.PetCalibrationRuntime as unknown as (new (options: Record<string, unknown>) => {
      activate(petId: string): Promise<void>;
      savedCalibration(): PetCalibrationV1;
    }) | undefined;
    expect(Runtime).toEqual(expect.any(Function));
    if (!Runtime) return;
    let activePetId = "pet-a";
    let releaseA!: (value: PetCalibrationV1) => void;
    const petA = new Promise<PetCalibrationV1>((resolve) => { releaseA = resolve; });
    const setCalibration = vi.fn();
    const runtime = new Runtime({
      activePetId: () => activePetId,
      load: (petId: string) => petId === "pet-a"
        ? petA
        : Promise.resolve(calibration({ feedbackStrength: 0.2 })),
      setCalibration,
      diagnose: vi.fn(),
    });

    const oldLoad = runtime.activate("pet-a");
    activePetId = "pet-b";
    const newLoad = runtime.activate("pet-b");
    expect(setCalibration).toHaveBeenLastCalledWith(DEFAULT_PET_CALIBRATION);
    await newLoad;
    releaseA(calibration({ feedbackStrength: 0.9 }));
    await oldLoad;

    expect(runtime.savedCalibration().feedbackStrength).toBe(0.2);
    expect(setCalibration).toHaveBeenLastCalledWith(calibration({ feedbackStrength: 0.2 }));
  });

  it("falls back safely to defaults when active-pet calibration loading fails", async () => {
    const Runtime = petStageModule.PetCalibrationRuntime as unknown as (new (options: Record<string, unknown>) => {
      activate(petId: string): Promise<void>;
      savedCalibration(): PetCalibrationV1;
    }) | undefined;
    expect(Runtime).toEqual(expect.any(Function));
    if (!Runtime) return;
    const diagnose = vi.fn();
    const setCalibration = vi.fn();
    const runtime = new Runtime({
      activePetId: () => "pet-a",
      load: async () => { throw new Error("corrupt calibration"); },
      setCalibration,
      diagnose,
    });

    await expect(runtime.activate("pet-a")).resolves.toBeUndefined();
    expect(runtime.savedCalibration()).toEqual(DEFAULT_PET_CALIBRATION);
    expect(setCalibration).toHaveBeenCalledWith(DEFAULT_PET_CALIBRATION);
    expect(diagnose).toHaveBeenCalledWith("calibration-load", expect.any(Error));
  });

  it("does not advance the saved snapshot when applying a loaded calibration fails", async () => {
    const Runtime = petStageModule.PetCalibrationRuntime as unknown as (new (options: Record<string, unknown>) => {
      activate(petId: string): Promise<void>;
      savedCalibration(): PetCalibrationV1;
    }) | undefined;
    expect(Runtime).toEqual(expect.any(Function));
    if (!Runtime) return;
    const setCalibration = vi.fn()
      .mockImplementationOnce(() => undefined)
      .mockImplementationOnce(() => { throw new Error("renderer rejected calibration"); });
    const runtime = new Runtime({
      activePetId: () => "pet-a",
      load: async () => calibration({ feedbackStrength: 1 }),
      setCalibration,
      diagnose: vi.fn(),
    });

    await runtime.activate("pet-a");

    expect(runtime.savedCalibration()).toEqual(DEFAULT_PET_CALIBRATION);
  });
});

describe("display scale protocol contracts", () => {
  it("accepts only safe request ids and finite in-range scales", () => {
    const guard = contracts.isPetDisplayScaleRequest as ((value: unknown) => boolean) | undefined;
    expect(guard).toEqual(expect.any(Function));
    if (!guard) return;

    expect(guard({ requestId: "scale-request_1:pet", displayScale: 1.25 })).toBe(true);
    for (const value of [
      { requestId: "", displayScale: 1 },
      { requestId: " has-space", displayScale: 1 },
      { requestId: "bad\nline", displayScale: 1 },
      { requestId: "a".repeat(129), displayScale: 1 },
      { requestId: "request-1", displayScale: Number.NaN },
      { requestId: "request-1", displayScale: Number.POSITIVE_INFINITY },
      { requestId: "request-1", displayScale: 0.49 },
      { requestId: "request-1", displayScale: 1.51 },
      { requestId: "request-1", displayScale: 1, extra: true },
    ]) expect(guard(value)).toBe(false);
  });

  it("validates the exact result union and actual rectangle", () => {
    const guard = contracts.isPetDisplayScaleResult as ((value: unknown) => boolean) | undefined;
    expect(guard).toEqual(expect.any(Function));
    if (!guard) return;

    expect(guard({
      requestId: "request-1",
      ok: true,
      requestedDisplayScale: 1.25,
      displayScale: 1.25,
      rect: { x: -1280.5, y: 12.25, width: 525, height: 650 },
    })).toBe(true);
    expect(guard({
      requestId: "request-1",
      ok: false,
      requestedDisplayScale: 1.25,
      message: "save failed",
    })).toBe(true);
    for (const value of [
      { requestId: "request-1", ok: true, displayScale: 1.25, rect: { x: 0, y: 0, width: 525, height: 650 } },
      { requestId: "request-1", ok: true, requestedDisplayScale: 1.25, displayScale: 1.25 },
      { requestId: "request-1", ok: true, requestedDisplayScale: 1.25, displayScale: 1.25, rect: { x: 0, y: 0, width: 0, height: 1 } },
      { requestId: "request-1", ok: true, requestedDisplayScale: 1.25, displayScale: 1.25, rect: { x: Number.NaN, y: 0, width: 1, height: 1 } },
      { requestId: "request-1", ok: false, requestedDisplayScale: 1.25, message: "" },
      { requestId: "request-1", ok: false, requestedDisplayScale: 1.25, message: "failed", rect: { x: 0, y: 0, width: 1, height: 1 } },
    ]) expect(guard(value)).toBe(false);
  });
});

function displayScaleClientHarness() {
  let handler: ((value: unknown) => void) | undefined;
  const unlisten = vi.fn<() => void>();
  const emit = vi.fn<(value: unknown) => Promise<void>>(async () => undefined);
  return {
    emit,
    ports: {
      listen: vi.fn(async (next: (value: unknown) => void) => { handler = next; return unlisten; }),
      emit,
    },
    result: (value: unknown) => handler?.(value),
    unlisten,
  };
}

describe("requestPetDisplayScale", () => {
  it("filters wrong and malformed acknowledgements and cleans up after the matching result", async () => {
    const request = bridge.requestPetDisplayScale as ((
      scale: number,
      options: Record<string, unknown>,
    ) => Promise<unknown>) | undefined;
    expect(request).toEqual(expect.any(Function));
    if (!request) return;
    const test = displayScaleClientHarness();

    const pending = request(1.25, {
      ports: test.ports,
      requestIdFactory: () => "request-1",
      timeoutMs: 5_000,
    });
    await vi.waitFor(() => expect(test.emit).toHaveBeenCalledOnce());
    test.result({ requestId: "request-2", ok: false, requestedDisplayScale: 1.25, message: "wrong request" });
    test.result({ requestId: "request-1", ok: false, requestedDisplayScale: 0.75, message: "wrong scale" });
    test.result({ requestId: "request-1", ok: true, requestedDisplayScale: 1.25, displayScale: 1.25, rect: { x: 0, y: 0, width: 0, height: 1 } });
    expect(test.unlisten).not.toHaveBeenCalled();
    const result = { requestId: "request-1", ok: true, requestedDisplayScale: 1.25, displayScale: 1.2, rect: { x: -200, y: 10, width: 504, height: 624 } };
    test.result(result);

    await expect(pending).resolves.toEqual(result);
    expect(test.unlisten).toHaveBeenCalledOnce();
  });

  it("rejects immediately when request emission fails", async () => {
    const request = bridge.requestPetDisplayScale as ((scale: number, options: Record<string, unknown>) => Promise<unknown>) | undefined;
    expect(request).toEqual(expect.any(Function));
    if (!request) return;
    const test = displayScaleClientHarness();
    test.emit.mockRejectedValue(new Error("pet window unavailable"));

    await expect(request(1, { ports: test.ports, requestIdFactory: () => "request-emit", timeoutMs: 5_000 }))
      .rejects.toThrow("pet window unavailable");
    expect(test.unlisten).toHaveBeenCalledOnce();
  });

  it("times out at five seconds, ignores a late ack, and unlistens a late listener", async () => {
    vi.useFakeTimers();
    const request = bridge.requestPetDisplayScale as ((scale: number, options: Record<string, unknown>) => Promise<unknown>) | undefined;
    expect(request).toEqual(expect.any(Function));
    if (!request) {
      vi.useRealTimers();
      return;
    }
    const test = displayScaleClientHarness();
    let resolveListen!: (unlisten: typeof test.unlisten) => void;
    test.ports.listen = vi.fn(() => new Promise((resolve) => { resolveListen = resolve; }));

    const pending = request(1, { ports: test.ports, requestIdFactory: () => "request-timeout", timeoutMs: 5_000 });
    const rejected = expect(pending).rejects.toThrow("5 seconds");
    await vi.advanceTimersByTimeAsync(5_000);
    await rejected;
    resolveListen(test.unlisten);
    await Promise.resolve();
    await Promise.resolve();

    expect(test.unlisten).toHaveBeenCalledOnce();
    expect(test.emit).not.toHaveBeenCalled();
    test.result({ requestId: "request-timeout", ok: false, requestedDisplayScale: 1, message: "late" });
    vi.useRealTimers();
  });

  it("creates a distinct id for every request", async () => {
    const request = bridge.requestPetDisplayScale as ((scale: number, options: Record<string, unknown>) => Promise<unknown>) | undefined;
    expect(request).toEqual(expect.any(Function));
    if (!request) return;
    const first = displayScaleClientHarness();
    const second = displayScaleClientHarness();
    let nextId = 0;
    const requestIdFactory = () => `request-${++nextId}`;

    const firstPending = request(1, { ports: first.ports, requestIdFactory, timeoutMs: 5_000 });
    const secondPending = request(1.25, { ports: second.ports, requestIdFactory, timeoutMs: 5_000 });
    await vi.waitFor(() => expect(first.emit).toHaveBeenCalledOnce());
    await vi.waitFor(() => expect(second.emit).toHaveBeenCalledOnce());
    const firstRequest = first.emit.mock.calls[0]?.[0] as { requestId: string };
    const secondRequest = second.emit.mock.calls[0]?.[0] as { requestId: string };
    expect(firstRequest.requestId).not.toBe(secondRequest.requestId);
    first.result({ requestId: firstRequest.requestId, ok: false, requestedDisplayScale: 1, message: "done" });
    second.result({ requestId: secondRequest.requestId, ok: false, requestedDisplayScale: 1.25, message: "done" });
    await Promise.all([firstPending, secondPending]);
  });

  it("rejects an active request id reuse before listen or emit and releases it after settlement", async () => {
    const request = bridge.requestPetDisplayScale as ((scale: number, options: Record<string, unknown>) => Promise<unknown>) | undefined;
    expect(request).toEqual(expect.any(Function));
    if (!request) return;
    const first = displayScaleClientHarness();
    const conflicting = displayScaleClientHarness();
    const reused = displayScaleClientHarness();
    const requestIdFactory = () => "shared-request";

    const firstPending = request(1.25, { ports: first.ports, requestIdFactory, timeoutMs: 5_000 });
    await vi.waitFor(() => expect(first.emit).toHaveBeenCalledOnce());
    await expect(request(0.75, { ports: conflicting.ports, requestIdFactory, timeoutMs: 5_000 }))
      .rejects.toThrow("already active");
    expect(conflicting.ports.listen).not.toHaveBeenCalled();
    expect(conflicting.emit).not.toHaveBeenCalled();

    first.result({
      requestId: "shared-request",
      ok: true,
      requestedDisplayScale: 1.25,
      displayScale: 1.2,
      rect: { x: 0, y: 0, width: 504, height: 624 },
    });
    await firstPending;

    const reusedPending = request(0.75, { ports: reused.ports, requestIdFactory, timeoutMs: 5_000 });
    await vi.waitFor(() => expect(reused.emit).toHaveBeenCalledOnce());
    reused.result({
      requestId: "shared-request",
      ok: false,
      requestedDisplayScale: 0.75,
      message: "settled",
    });
    await reusedPending;
  });
});

describe("logical Tauri window size port", () => {
  it("converts negative mixed-DPI physical geometry once at the boundary", async () => {
    const createPort = bridge.createLogicalWindowSizePort as ((options: Record<string, unknown>) => {
      getRect(): Promise<unknown>;
      getWorkArea(): Promise<unknown>;
      setRect(rect: unknown): Promise<void>;
    }) | undefined;
    expect(createPort).toEqual(expect.any(Function));
    if (!createPort) return;
    const setPosition = vi.fn<(value: unknown) => Promise<void>>(async () => undefined);
    const setSize = vi.fn<(value: unknown) => Promise<void>>(async () => undefined);
    const port = createPort({
      window: {
        scaleFactor: async () => 2,
        outerPosition: async () => new PhysicalPosition(-2400, 200),
        outerSize: async () => new PhysicalSize(840, 1040),
        setPosition,
        setSize,
      },
      currentMonitor: async () => ({
        scaleFactor: 1.5,
        workArea: {
          position: new PhysicalPosition(-1920, -150),
          size: new PhysicalSize(1920, 1536),
        },
      }),
      resizeRenderer: async () => undefined,
      refreshHitRegion: async () => undefined,
    });

    await expect(port.getRect()).resolves.toEqual({ x: -1200, y: 100, width: 420, height: 520 });
    await expect(port.getWorkArea()).resolves.toEqual({ x: -1280, y: -100, width: 1280, height: 1024 });
    await port.setRect({ x: -1280, y: -100, width: 525, height: 650 });

    expect(setSize.mock.calls[0]?.[0]).toMatchObject({ type: "Logical", width: 525, height: 650 });
    expect(setPosition.mock.calls[0]?.[0]).toMatchObject({ type: "Logical", x: -1280, y: -100 });
  });

  it("does not show or focus a hidden window while resizing", async () => {
    const createPort = bridge.createLogicalWindowSizePort as ((options: Record<string, unknown>) => { setRect(rect: unknown): Promise<void> }) | undefined;
    expect(createPort).toEqual(expect.any(Function));
    if (!createPort) return;
    const show = vi.fn(async () => undefined);
    const setFocus = vi.fn(async () => undefined);
    const port = createPort({
      window: {
        scaleFactor: async () => 1,
        outerPosition: async () => new PhysicalPosition(0, 0),
        outerSize: async () => new PhysicalSize(420, 520),
        setPosition: vi.fn(async () => undefined),
        setSize: vi.fn(async () => undefined),
        show,
        setFocus,
      },
      currentMonitor: async () => null,
      resizeRenderer: async () => undefined,
      refreshHitRegion: async () => undefined,
    });

    await port.setRect({ x: 0, y: 0, width: 525, height: 650 });

    expect(show).not.toHaveBeenCalled();
    expect(setFocus).not.toHaveBeenCalled();
  });
});

describe("logical v2 window geometry persistence", () => {
  it("persists a DPI-2 negative-screen physical rect as logical v2 geometry", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: 0,
      y: 0,
      width: 420,
      height: 520,
      displayScale: 1.25,
      flipped: false,
      mode: "companion",
    };
    const save = vi.fn(async () => undefined);
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 2,
        outerPosition: async () => new PhysicalPosition(999, 999),
        outerSize: async () => new PhysicalSize(999, 999),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });

    await persistence.persist({
      position: new PhysicalPosition(-2400, 200),
      size: new PhysicalSize(840, 1040),
    });

    expect(preferences).toMatchObject({
      x: -1200,
      y: 100,
      width: 420,
      height: 520,
      displayScale: 1.25,
    });
    expect(save).toHaveBeenCalledWith({ ...preferences });
  });

  it("uses the current window scale factor and leaves preferences unchanged when it is invalid", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: -50,
      y: 20,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    const old = { ...preferences };
    const save = vi.fn(async () => undefined);
    const diagnose = vi.fn();
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 0,
        outerPosition: async () => new PhysicalPosition(-2400, 200),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save,
      diagnose,
    });

    await persistence.persist();

    expect(preferences).toEqual(old);
    expect(save).not.toHaveBeenCalled();
    expect(diagnose).toHaveBeenCalledWith("window-geometry", expect.any(RangeError));
  });

  it("drops stale async reads and serializes saves so an older event cannot win", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    let resolveFirstScale!: (scale: number) => void;
    const firstScale = new Promise<number>((resolve) => { resolveFirstScale = resolve; });
    let scaleCalls = 0;
    const preferences = {
      schemaVersion: 2,
      x: 0,
      y: 0,
      width: 420,
      height: 520,
      displayScale: 1.5,
      flipped: false,
      mode: "companion",
    };
    const save = vi.fn(async () => undefined);
    const persistence = createPersistence({
      window: {
        scaleFactor: vi.fn(async () => {
          scaleCalls += 1;
          return scaleCalls === 1 ? firstScale : 2;
        }),
        outerPosition: async () => new PhysicalPosition(0, 0),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });

    const stale = persistence.persist({
      position: new PhysicalPosition(-2400, 200),
      size: new PhysicalSize(840, 1040),
    });
    const latest = persistence.persist({
      position: new PhysicalPosition(-2000, 400),
      size: new PhysicalSize(630, 780),
    });
    await latest;
    resolveFirstScale(2);
    await stale;

    expect(save).toHaveBeenCalledOnce();
    expect(preferences).toMatchObject({
      x: -1000,
      y: 200,
      width: 315,
      height: 390,
      displayScale: 1.5,
    });
  });

  it("serializes resize persistence before scale commit so old displayScale cannot land last", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: 0,
      y: 0,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    let releaseFirstSave!: () => void;
    const firstSaveGate = new Promise<void>((resolve) => { releaseFirstSave = resolve; });
    const saved: Array<Record<string, unknown>> = [];
    const save = vi.fn(async (value: Record<string, unknown>) => {
      saved.push({ ...value });
      if (saved.length === 1) await firstSaveGate;
    });
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 2,
        outerPosition: async () => new PhysicalPosition(-2400, 200),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });

    const resizeSave = persistence.persist();
    await vi.waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(persistence.runDisplayScaleTransaction).toEqual(expect.any(Function));
    if (!persistence.runDisplayScaleTransaction) return;
    const scaleCommit = persistence.runDisplayScaleTransaction(() => persistence.commitDisplayScale({
      requestedScale: 1.25,
      appliedScale: 1.2,
      rect: { x: -1200, y: 100, width: 504, height: 624 },
    }));
    await Promise.resolve();
    expect(save).toHaveBeenCalledOnce();
    releaseFirstSave();
    await Promise.all([resizeSave, scaleCommit]);

    expect(saved).toEqual([
      expect.objectContaining({ displayScale: 1, x: -1200, y: 100, width: 420, height: 520 }),
      expect.objectContaining({ displayScale: 1.2, x: -1200, y: 100, width: 504, height: 624 }),
    ]);
    expect(preferences).toMatchObject({ displayScale: 1.2, width: 504, height: 624 });
  });

  it("restores the last successful geometry baseline when a queued scale save fails", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: 0,
      y: 0,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    let disk = { ...preferences };
    let saveCalls = 0;
    let enterGeometrySave!: () => void;
    const geometrySaveEntered = new Promise<void>((resolve) => { enterGeometrySave = resolve; });
    let releaseGeometrySave!: () => void;
    const geometrySaveGate = new Promise<void>((resolve) => { releaseGeometrySave = resolve; });
    const scaleSaveError = new Error("scale save failed");
    const save = vi.fn(async (value: typeof preferences) => {
      saveCalls += 1;
      if (saveCalls === 1) {
        enterGeometrySave();
        await geometrySaveGate;
        disk = { ...value };
        return;
      }
      if (saveCalls === 2) throw scaleSaveError;
      disk = { ...value };
    });
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 2,
        outerPosition: async () => new PhysicalPosition(200, 400),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });
    const geometry = persistence.persist();
    await geometrySaveEntered;

    const originalRect = { x: 100, y: 200, width: 420, height: 520 };
    let currentRect = { ...originalRect };
    const port: WindowSizePort = {
      getRect: vi.fn(async () => ({ ...currentRect })),
      getWorkArea: vi.fn(async () => ({ x: -1280, y: -100, width: 2560, height: 1440 })),
      setRect: vi.fn(async (rect) => { currentRect = { ...rect }; }),
      resizeRenderer: vi.fn(async () => undefined),
      refreshHitRegion: vi.fn(async () => undefined),
    };
    const controller = new WindowSizeController(port);
    expect(persistence.runDisplayScaleTransaction).toEqual(expect.any(Function));
    if (!persistence.runDisplayScaleTransaction) return;
    const scale = persistence.runDisplayScaleTransaction(
      () => controller.apply(1.25, (ack) => persistence.commitDisplayScale(ack)),
    );
    const scaleRejected = scale.catch((error: unknown) => error);
    await Promise.resolve();
    expect(port.getRect).not.toHaveBeenCalled();
    releaseGeometrySave();
    await geometry;
    await vi.waitFor(() => expect(port.refreshHitRegion).toHaveBeenCalled());
    expect(await scaleRejected).toBe(scaleSaveError);

    const geometryBaseline = {
      ...preferences,
      x: 100,
      y: 200,
      width: 420,
      height: 520,
      displayScale: 1,
    };
    expect(disk).toEqual(geometryBaseline);
    expect(preferences).toEqual(geometryBaseline);
    expect(currentRect).toEqual(originalRect);

    await persistence.persist({
      position: new PhysicalPosition(240, 440),
      size: new PhysicalSize(840, 1040),
    });
    expect(disk).toMatchObject({ x: 120, y: 220, width: 420, height: 520, displayScale: 1 });
    await persistence.runDisplayScaleTransaction(() => persistence.commitDisplayScale({
      requestedScale: 0.75,
      appliedScale: 0.75,
      rect: { x: 172, y: 350, width: 315, height: 390 },
    }));
    expect(disk).toMatchObject({ x: 172, y: 350, width: 315, height: 390, displayScale: 0.75 });
    expect(preferences).toEqual(disk);
  });

  it("publishes a successful in-flight geometry save even when a newer read never saves", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: 0,
      y: 0,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    let disk = { ...preferences };
    let enterSave!: () => void;
    const saveEntered = new Promise<void>((resolve) => { enterSave = resolve; });
    let releaseSave!: () => void;
    const saveGate = new Promise<void>((resolve) => { releaseSave = resolve; });
    let scaleCalls = 0;
    let rejectNewRead!: (error: Error) => void;
    const newRead = new Promise<number>((_resolve, reject) => { rejectNewRead = reject; });
    const persistence = createPersistence({
      window: {
        scaleFactor: vi.fn(async () => {
          scaleCalls += 1;
          return scaleCalls === 1 ? 2 : newRead;
        }),
        outerPosition: async () => new PhysicalPosition(200, 400),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save: vi.fn(async (value: typeof preferences) => {
        enterSave();
        await saveGate;
        disk = { ...value };
      }),
      diagnose: vi.fn(),
    });

    const successful = persistence.persist();
    await saveEntered;
    const neverSaved = persistence.persist();
    releaseSave();
    await successful;

    expect(disk).toMatchObject({ x: 100, y: 200, width: 420, height: 520, displayScale: 1 });
    expect(preferences).toEqual(disk);

    rejectNewRead(new Error("app is closing"));
    await neverSaved;
    expect(preferences).toEqual(disk);
  });

  it("suppresses geometry emitted during a scale save instead of replaying an intermediate rect", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: 0,
      y: 0,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    const saved: Array<typeof preferences> = [];
    let releaseScaleSave!: () => void;
    const scaleSaveGate = new Promise<void>((resolve) => { releaseScaleSave = resolve; });
    const save = vi.fn(async (value: typeof preferences) => {
      saved.push({ ...value });
      if (saved.length === 1) await scaleSaveGate;
    });
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 2,
        outerPosition: async () => new PhysicalPosition(240, 440),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });
    expect(persistence.runDisplayScaleTransaction).toEqual(expect.any(Function));
    if (!persistence.runDisplayScaleTransaction) return;

    const scale = persistence.runDisplayScaleTransaction(() => persistence.commitDisplayScale({
      requestedScale: 1.25,
      appliedScale: 1.2,
      rect: { x: 100, y: 200, width: 504, height: 624 },
    }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledOnce());
    const geometry = persistence.persist();
    releaseScaleSave();
    await Promise.all([scale, geometry]);

    expect(saved).toEqual([
      expect.objectContaining({ displayScale: 1.2, x: 100, y: 200, width: 504, height: 624 }),
    ]);
    expect(preferences).toEqual(saved[0]);
  });

  it("keeps disk, memory, and window at P0 when resize events precede a failed scale commit", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const p0 = {
      schemaVersion: 2,
      x: 100,
      y: 200,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    const preferences = { ...p0 };
    let disk = { ...p0 };
    const scaleSaveError = new Error("scale save failed");
    const save = vi.fn(async (value: typeof preferences) => {
      if (value.displayScale !== p0.displayScale) throw scaleSaveError;
      disk = { ...value };
    });
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 2,
        outerPosition: async () => new PhysicalPosition(200, 400),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });
    let currentRect = { x: p0.x, y: p0.y, width: p0.width, height: p0.height };
    const port: WindowSizePort = {
      getRect: vi.fn(async () => ({ ...currentRect })),
      getWorkArea: vi.fn(async () => ({ x: 0, y: 0, width: 1920, height: 1080 })),
      setRect: vi.fn(async (rect) => {
        currentRect = { ...rect };
        await persistence.persist({
          position: new PhysicalPosition(rect.x * 2, rect.y * 2),
          size: new PhysicalSize(rect.width * 2, rect.height * 2),
        });
      }),
      resizeRenderer: vi.fn(async () => undefined),
      refreshHitRegion: vi.fn(async () => undefined),
    };
    const controller = new WindowSizeController(port);
    expect(persistence.runDisplayScaleTransaction).toEqual(expect.any(Function));
    if (!persistence.runDisplayScaleTransaction) return;

    await expect(persistence.runDisplayScaleTransaction(
      () => controller.apply(1.25, (ack) => persistence.commitDisplayScale(ack)),
    )).rejects.toBe(scaleSaveError);

    expect(save).toHaveBeenCalledOnce();
    expect(disk).toEqual(p0);
    expect(preferences).toEqual(p0);
    expect(currentRect).toEqual({ x: p0.x, y: p0.y, width: p0.width, height: p0.height });
  });

  it("persists one coherent actual rect and scale on a successful scale transaction", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: 100,
      y: 200,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    const save = vi.fn(async () => undefined);
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 2,
        outerPosition: async () => new PhysicalPosition(200, 400),
        outerSize: async () => new PhysicalSize(840, 1040),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });
    let currentRect = { x: 100, y: 200, width: 420, height: 520 };
    const actualRect = { x: 48, y: 70, width: 504, height: 624 };
    const port: WindowSizePort = {
      getRect: vi.fn(async () => ({ ...currentRect })),
      getWorkArea: vi.fn(async () => ({ x: 0, y: 0, width: 1920, height: 1080 })),
      setRect: vi.fn(async (rect) => {
        currentRect = rect.width === 525 ? { ...actualRect } : { ...rect };
        await persistence.persist({
          position: new PhysicalPosition(currentRect.x * 2, currentRect.y * 2),
          size: new PhysicalSize(currentRect.width * 2, currentRect.height * 2),
        });
      }),
      resizeRenderer: vi.fn(async () => undefined),
      refreshHitRegion: vi.fn(async () => undefined),
    };
    const controller = new WindowSizeController(port);
    expect(persistence.runDisplayScaleTransaction).toEqual(expect.any(Function));
    if (!persistence.runDisplayScaleTransaction) return;

    const ack = await persistence.runDisplayScaleTransaction(
      () => controller.apply(1.25, (value) => persistence.commitDisplayScale(value)),
    );

    expect(ack.rect).toEqual(actualRect);
    expect(save).toHaveBeenCalledOnce();
    expect(save).toHaveBeenCalledWith(expect.objectContaining({
      x: actualRect.x,
      y: actualRect.y,
      width: actualRect.width,
      height: actualRect.height,
      displayScale: ack.appliedScale,
    }));
    expect(preferences).toMatchObject({ ...actualRect, displayScale: ack.appliedScale });
  });

  it.each(["renderer", "readback"] as const)(
    "does not persist resize events when scale %s fails before commit",
    async (failureStage) => {
      const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
        persist(geometry?: Record<string, unknown>): Promise<void>;
        commitDisplayScale(ack: WindowSizeAck): Promise<void>;
        runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
      }) | undefined;
      expect(createPersistence).toEqual(expect.any(Function));
      if (!createPersistence) return;
      const preferences = {
        schemaVersion: 2,
        x: 100,
        y: 200,
        width: 420,
        height: 520,
        displayScale: 1,
        flipped: false,
        mode: "companion",
      };
      const save = vi.fn(async () => undefined);
      const persistence = createPersistence({
        window: {
          scaleFactor: async () => 2,
          outerPosition: async () => new PhysicalPosition(200, 400),
          outerSize: async () => new PhysicalSize(840, 1040),
        },
        preferences,
        save,
        diagnose: vi.fn(),
      });
      const originalRect = { x: 100, y: 200, width: 420, height: 520 };
      let currentRect = { ...originalRect };
      let getRectCalls = 0;
      let rendererCalls = 0;
      const failure = new Error(`${failureStage} failed`);
      const port: WindowSizePort = {
        getRect: vi.fn(async () => {
          getRectCalls += 1;
          if (failureStage === "readback" && getRectCalls === 2) throw failure;
          return { ...currentRect };
        }),
        getWorkArea: vi.fn(async () => ({ x: 0, y: 0, width: 1920, height: 1080 })),
        setRect: vi.fn(async (rect) => {
          currentRect = { ...rect };
          await persistence.persist({
            position: new PhysicalPosition(rect.x * 2, rect.y * 2),
            size: new PhysicalSize(rect.width * 2, rect.height * 2),
          });
        }),
        resizeRenderer: vi.fn(async () => {
          rendererCalls += 1;
          if (failureStage === "renderer" && rendererCalls === 1) throw failure;
        }),
        refreshHitRegion: vi.fn(async () => undefined),
      };
      const controller = new WindowSizeController(port);
      expect(persistence.runDisplayScaleTransaction).toEqual(expect.any(Function));
      if (!persistence.runDisplayScaleTransaction) return;

      await expect(persistence.runDisplayScaleTransaction(
        () => controller.apply(1.25, (ack) => persistence.commitDisplayScale(ack)),
      )).rejects.toBe(failure);

      expect(save).not.toHaveBeenCalled();
      expect(preferences).toMatchObject({ ...originalRect, displayScale: 1 });
      expect(currentRect).toEqual(originalRect);
    },
  );

  it("rejects nested scale transactions and releases suppression after an exception", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const preferences = {
      schemaVersion: 2,
      x: 0,
      y: 0,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    const save = vi.fn(async () => undefined);
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 1,
        outerPosition: async () => new PhysicalPosition(20, 40),
        outerSize: async () => new PhysicalSize(420, 520),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });
    const operationError = new Error("operation failed");

    await expect(persistence.runDisplayScaleTransaction(async () => {
      await expect(persistence.runDisplayScaleTransaction(async () => undefined))
        .rejects.toThrow("already active");
      throw operationError;
    })).rejects.toBe(operationError);

    await persistence.persist();
    expect(save).toHaveBeenCalledOnce();
    expect(preferences).toMatchObject({ x: 20, y: 40, width: 420, height: 520 });
  });

  it("freezes motion before draining a blocked geometry save so a new drag cannot create H", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      flushCurrentGeometry(): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    const coordinate = (windowMotionModule as unknown as {
      runWithWindowMotionSuspended?: <T>(
        motion: WindowMotionController,
        flush: () => Promise<void>,
        operation: () => Promise<T>,
      ) => Promise<T>;
    }).runWithWindowMotionSuspended;
    expect(createPersistence).toEqual(expect.any(Function));
    expect(coordinate).toEqual(expect.any(Function));
    if (!createPersistence || !coordinate) return;
    const g = { x: 120, y: 220, width: 420, height: 520 };
    const preferences = {
      schemaVersion: 2,
      ...g,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    let currentRect = { ...g };
    let releaseFirstSave!: () => void;
    const firstSaveBlocked = new Promise<void>((resolve) => { releaseFirstSave = resolve; });
    let saveCalls = 0;
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 1,
        outerPosition: async () => new PhysicalPosition(currentRect.x, currentRect.y),
        outerSize: async () => new PhysicalSize(currentRect.width, currentRect.height),
      },
      preferences,
      save: vi.fn(async () => {
        saveCalls += 1;
        if (saveCalls === 1) await firstSaveBlocked;
      }),
      diagnose: vi.fn(),
    });
    const motion = new WindowMotionController({
      getPosition: async () => ({ x: currentRect.x, y: currentRect.y }),
      setPosition: async (position) => {
        currentRect = { ...currentRect, ...position };
      },
      persistPosition: (position) => persistence.persist({
        position: new PhysicalPosition(position.x, position.y),
      }),
    });
    const gSave = persistence.persist();
    await vi.waitFor(() => expect(saveCalls).toBe(1));
    let scaleStarted = false;
    const scale = coordinate(
      motion,
      () => persistence.flushCurrentGeometry(),
      async () => {
        scaleStarted = true;
        return persistence.runDisplayScaleTransaction(async () => undefined);
      },
    );

    await motion.beginDrag({ x: 0, y: 0 });
    await motion.dragTo({ x: 80, y: 80 });
    expect(currentRect).toEqual(g);
    expect(scaleStarted).toBe(false);
    releaseFirstSave();
    await gSave;
    await scale;

    expect(currentRect).toEqual(g);
  });

  it("propagates an explicit geometry flush failure instead of starting scale", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      flushCurrentGeometry?: () => Promise<void>;
    }) | undefined;
    expect(createPersistence).toEqual(expect.any(Function));
    if (!createPersistence) return;
    const saveError = new Error("flush save failed");
    const preferences = {
      schemaVersion: 2,
      x: 100,
      y: 200,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 1,
        outerPosition: async () => new PhysicalPosition(100, 200),
        outerSize: async () => new PhysicalSize(420, 520),
      },
      preferences,
      save: async () => { throw saveError; },
      diagnose: vi.fn(),
    });

    expect(persistence.flushCurrentGeometry).toEqual(expect.any(Function));
    if (!persistence.flushCurrentGeometry) return;
    await expect(persistence.flushCurrentGeometry()).rejects.toBe(saveError);
    expect(preferences).toMatchObject({ x: 100, y: 200, width: 420, height: 520, displayScale: 1 });
  });

  it("waits for in-flight motion to H and flushes H before a failed scale rolls back", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      persist(geometry?: Record<string, unknown>): Promise<void>;
      flushCurrentGeometry(): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    const coordinate = (windowMotionModule as unknown as {
      runWithWindowMotionSuspended?: <T>(
        motion: WindowMotionController,
        flush: () => Promise<void>,
        operation: () => Promise<T>,
      ) => Promise<T>;
    }).runWithWindowMotionSuspended;
    expect(createPersistence).toEqual(expect.any(Function));
    expect(coordinate).toEqual(expect.any(Function));
    if (!createPersistence || !coordinate) return;
    const g = { x: 100, y: 200, width: 420, height: 520 };
    const h = { x: 140, y: 260, width: 420, height: 520 };
    const preferences = {
      schemaVersion: 2,
      ...g,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    let currentRect = { ...g };
    let disk = { ...preferences };
    const scaleSaveError = new Error("scale save failed");
    const save = vi.fn(async (value: typeof preferences) => {
      if (value.displayScale !== 1) throw scaleSaveError;
      disk = { ...value };
    });
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 1,
        outerPosition: async () => new PhysicalPosition(currentRect.x, currentRect.y),
        outerSize: async () => new PhysicalSize(currentRect.width, currentRect.height),
      },
      preferences,
      save,
      diagnose: vi.fn(),
    });
    let releaseMove!: () => void;
    const moveBlocked = new Promise<void>((resolve) => { releaseMove = resolve; });
    const motion = new WindowMotionController({
      getPosition: async () => ({ x: currentRect.x, y: currentRect.y }),
      setPosition: async (position) => {
        await moveBlocked;
        currentRect = { ...currentRect, ...position };
      },
      persistPosition: (position) => persistence.persist({
        position: new PhysicalPosition(position.x, position.y),
      }),
    });
    const show = vi.fn(async () => undefined);
    const sizePort: WindowSizePort & { show(): Promise<void> } = {
      getRect: vi.fn(async () => ({ ...currentRect })),
      getWorkArea: vi.fn(async () => ({ x: 0, y: 0, width: 1920, height: 1080 })),
      setRect: vi.fn(async (rect) => {
        currentRect = { ...rect };
        await persistence.persist({
          position: new PhysicalPosition(rect.x, rect.y),
          size: new PhysicalSize(rect.width, rect.height),
        });
      }),
      resizeRenderer: vi.fn(async () => undefined),
      refreshHitRegion: vi.fn(async () => undefined),
      show,
    };
    const size = new WindowSizeController(sizePort);
    await motion.beginDrag({ x: 0, y: 0 });
    const moveToH = motion.dragTo({ x: h.x - g.x, y: h.y - g.y });
    const scale = coordinate(
      motion,
      () => persistence.flushCurrentGeometry(),
      () => persistence.runDisplayScaleTransaction(
        () => size.apply(1.25, (ack) => persistence.commitDisplayScale(ack)),
      ),
    );
    await Promise.resolve();
    expect(sizePort.getRect).not.toHaveBeenCalled();

    releaseMove();
    await moveToH;
    await expect(scale).rejects.toBe(scaleSaveError);

    expect(currentRect).toEqual(h);
    expect(preferences).toEqual({ ...disk, ...h, displayScale: 1 });
    expect(disk).toMatchObject({ ...h, displayScale: 1 });
    expect(show).not.toHaveBeenCalled();
  });

  it("writes only one coherent scale snapshot after the coordinated geometry flush", async () => {
    const createPersistence = bridge.createLogicalWindowGeometryPersistence as ((options: Record<string, unknown>) => {
      flushCurrentGeometry(): Promise<void>;
      commitDisplayScale(ack: WindowSizeAck): Promise<void>;
      runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
    }) | undefined;
    const coordinate = (windowMotionModule as unknown as {
      runWithWindowMotionSuspended?: <T>(
        motion: WindowMotionController,
        flush: () => Promise<void>,
        operation: () => Promise<T>,
      ) => Promise<T>;
    }).runWithWindowMotionSuspended;
    expect(createPersistence).toEqual(expect.any(Function));
    expect(coordinate).toEqual(expect.any(Function));
    if (!createPersistence || !coordinate) return;
    const preferences = {
      schemaVersion: 2,
      x: 100,
      y: 200,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    let currentRect = { x: 100, y: 200, width: 420, height: 520 };
    const saved: Array<typeof preferences> = [];
    const persistence = createPersistence({
      window: {
        scaleFactor: async () => 1,
        outerPosition: async () => new PhysicalPosition(currentRect.x, currentRect.y),
        outerSize: async () => new PhysicalSize(currentRect.width, currentRect.height),
      },
      preferences,
      save: async (value: typeof preferences) => { saved.push({ ...value }); },
      diagnose: vi.fn(),
    });
    const motion = new WindowMotionController({
      getPosition: async () => ({ x: currentRect.x, y: currentRect.y }),
      setPosition: async (position) => { currentRect = { ...currentRect, ...position }; },
      persistPosition: async () => undefined,
    });
    const size = new WindowSizeController({
      getRect: async () => ({ ...currentRect }),
      getWorkArea: async () => ({ x: 0, y: 0, width: 1920, height: 1080 }),
      setRect: async (rect) => { currentRect = { ...rect }; },
      resizeRenderer: async () => undefined,
      refreshHitRegion: async () => undefined,
    });

    const ack = await coordinate(
      motion,
      () => persistence.flushCurrentGeometry(),
      () => persistence.runDisplayScaleTransaction(
        () => size.apply(1.25, (value) => persistence.commitDisplayScale(value)),
      ),
    );

    expect(saved.filter((value) => value.displayScale !== 1)).toEqual([{
      ...preferences,
      ...ack.rect,
      displayScale: ack.appliedScale,
    }]);
    expect(preferences).toMatchObject({ ...ack.rect, displayScale: ack.appliedScale });
  });
});

describe("pet display scale listener", () => {
  it("processes concurrent requests FIFO and emits only after apply commit finishes", async () => {
    const register = bridge.listenForPetDisplayScaleRequests as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    expect(register).toEqual(expect.any(Function));
    if (!register) return;
    let receive!: (value: unknown) => void;
    const events: string[] = [];
    let releaseFirst!: () => void;
    const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve; });
    const apply = vi.fn(async (scale: number, commit: (ack: unknown) => Promise<void>) => {
      events.push(`apply:${scale}`);
      if (scale === 1.25) await firstGate;
      const ack = { requestedScale: scale, appliedScale: scale, rect: { x: 0, y: 0, width: 420 * scale, height: 520 * scale } };
      events.push(`visual:${scale}`);
      await commit(ack);
      events.push(`saved:${scale}`);
      return ack;
    });
    const emit = vi.fn(async (result: { displayScale?: number }) => { events.push(`emit:${result.displayScale}`); });
    await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return vi.fn(); },
      emit,
      apply,
      commit: vi.fn(async () => undefined),
    });

    receive({ requestId: "request-1", displayScale: 1.25 });
    receive({ requestId: "request-2", displayScale: 0.75 });
    await Promise.resolve();
    expect(apply).toHaveBeenCalledTimes(1);
    releaseFirst();
    await vi.waitFor(() => expect(emit).toHaveBeenCalledTimes(2));

    expect(events).toEqual([
      "apply:1.25", "visual:1.25", "saved:1.25", "emit:1.25",
      "apply:0.75", "visual:0.75", "saved:0.75", "emit:0.75",
    ]);
    expect(emit.mock.calls[0]?.[0]).toMatchObject({ requestedDisplayScale: 1.25 });
    expect(emit.mock.calls[1]?.[0]).toMatchObject({ requestedDisplayScale: 0.75 });
  });

  it("deduplicates repeated request ids and survives apply and emit failures", async () => {
    const register = bridge.listenForPetDisplayScaleRequests as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    expect(register).toEqual(expect.any(Function));
    if (!register) return;
    let receive!: (value: unknown) => void;
    const emit = vi.fn()
      .mockRejectedValueOnce(new Error("settings closed"))
      .mockResolvedValue(undefined);
    const apply = vi.fn(async (scale: number, commit: (ack: unknown) => Promise<void>) => {
      if (scale === 1.25) throw new Error("resize failed");
      const ack = { requestedScale: scale, appliedScale: scale, rect: { x: 0, y: 0, width: 315, height: 390 } };
      await commit(ack);
      return ack;
    });
    const diagnose = vi.fn(() => { throw new Error("diagnostic sink failed"); });
    await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return vi.fn(); },
      emit,
      apply,
      commit: vi.fn(async () => undefined),
      diagnose,
    });

    receive({ requestId: "request-1", displayScale: 1.25 });
    receive({ requestId: "request-1", displayScale: 1.25 });
    receive({ requestId: "request-2", displayScale: 0.75 });
    await vi.waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(emit).toHaveBeenCalledTimes(2));

    expect(apply.mock.calls.map((call) => call[0])).toEqual([1.25, 0.75]);
    expect(emit.mock.calls[0]?.[0]).toMatchObject({ requestId: "request-1", ok: false, requestedDisplayScale: 1.25, message: "resize failed" });
    expect(emit.mock.calls[1]?.[0]).toMatchObject({ requestId: "request-2", ok: true, requestedDisplayScale: 0.75 });
    expect(diagnose).toHaveBeenCalledOnce();
  });

  it("caps failure messages so emitted results still satisfy the strict contract", async () => {
    const register = bridge.listenForPetDisplayScaleRequests as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    const resultGuard = contracts.isPetDisplayScaleResult as ((value: unknown) => boolean) | undefined;
    expect(register).toEqual(expect.any(Function));
    expect(resultGuard).toEqual(expect.any(Function));
    if (!register || !resultGuard) return;
    let receive!: (value: unknown) => void;
    const emit = vi.fn<(value: unknown) => Promise<void>>(async () => undefined);
    await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return vi.fn(); },
      emit,
      apply: vi.fn(async () => { throw new Error("x".repeat(3_000)); }),
      commit: vi.fn(async () => undefined),
    });

    receive({ requestId: "request-long-error", displayScale: 1 });
    await vi.waitFor(() => expect(emit).toHaveBeenCalledOnce());

    const result = emit.mock.calls[0]?.[0];
    expect(resultGuard(result)).toBe(true);
    expect((result as { message: string }).message).toHaveLength(2_048);
  });

  it("replays identical ids but rejects in-flight and completed scale conflicts without stale acks", async () => {
    const register = bridge.listenForPetDisplayScaleRequests as ((options: Record<string, unknown>) => Promise<() => void>) | undefined;
    expect(register).toEqual(expect.any(Function));
    if (!register) return;
    let receive!: (value: unknown) => void;
    let releaseApply!: () => void;
    const gate = new Promise<void>((resolve) => { releaseApply = resolve; });
    const apply = vi.fn(async (scale: number, commit: (ack: unknown) => Promise<void>) => {
      await gate;
      const ack = {
        requestedScale: scale,
        appliedScale: 1.2,
        rect: { x: 0, y: 0, width: 504, height: 624 },
      };
      await commit(ack);
      return ack;
    });
    const emit = vi.fn<(result: unknown) => Promise<void>>(async () => undefined);
    await register({
      listen: async (handler: (value: unknown) => void) => { receive = handler; return vi.fn(); },
      emit,
      apply,
      commit: vi.fn(async () => undefined),
    });

    receive({ requestId: "shared-id", displayScale: 1.25 });
    receive({ requestId: "shared-id", displayScale: 1.25 });
    receive({ requestId: "shared-id", displayScale: 0.75 });
    await vi.waitFor(() => expect(emit).toHaveBeenCalledOnce());
    expect(emit.mock.calls[0]?.[0]).toMatchObject({
      requestId: "shared-id",
      ok: false,
      requestedDisplayScale: 0.75,
    });
    releaseApply();
    await vi.waitFor(() => expect(emit).toHaveBeenCalledTimes(2));
    expect(emit.mock.calls[1]?.[0]).toMatchObject({
      requestId: "shared-id",
      ok: true,
      requestedDisplayScale: 1.25,
      displayScale: 1.2,
    });
    expect(apply).toHaveBeenCalledOnce();

    receive({ requestId: "shared-id", displayScale: 1.25 });
    receive({ requestId: "shared-id", displayScale: 0.5 });
    await vi.waitFor(() => expect(emit).toHaveBeenCalledTimes(4));
    expect(emit.mock.calls[2]?.[0]).toEqual(emit.mock.calls[1]?.[0]);
    expect(emit.mock.calls[3]?.[0]).toMatchObject({
      requestId: "shared-id",
      ok: false,
      requestedDisplayScale: 0.5,
    });
    expect(apply).toHaveBeenCalledOnce();
  });
});

describe("display scale preference commit", () => {
  it("persists the actual readback geometry before acknowledgement", async () => {
    const commit = bridge.commitDisplayScalePreferences as ((
      preferences: Record<string, unknown>,
      ack: Record<string, unknown>,
      save: (value: unknown) => Promise<void>,
    ) => Promise<void>) | undefined;
    expect(commit).toEqual(expect.any(Function));
    if (!commit) return;
    const preferences = {
      schemaVersion: 2,
      x: 10,
      y: 20,
      width: 420,
      height: 520,
      displayScale: 1,
      flipped: false,
      mode: "companion",
    };
    const save = vi.fn(async () => undefined);

    await commit(preferences, {
      requestedScale: 1.25,
      appliedScale: 1.2,
      rect: { x: -1200, y: 100, width: 504, height: 624 },
    }, save);

    expect(preferences).toMatchObject({
      displayScale: 1.2,
      x: -1200,
      y: 100,
      width: 504,
      height: 624,
    });
    expect(save).toHaveBeenCalledWith({ ...preferences });
  });

  it("restores the exact old in-memory preferences when atomic save fails", async () => {
    const commit = bridge.commitDisplayScalePreferences as ((
      preferences: Record<string, unknown>,
      ack: Record<string, unknown>,
      save: (value: unknown) => Promise<void>,
    ) => Promise<void>) | undefined;
    expect(commit).toEqual(expect.any(Function));
    if (!commit) return;
    const preferences = {
      schemaVersion: 2,
      x: -111,
      y: 27,
      width: 421,
      height: 519,
      displayScale: 1,
      flipped: true,
      mode: "desktop",
    };
    const old = { ...preferences };
    const saveError = new Error("disk full");

    await expect(commit(preferences, {
      requestedScale: 1.5,
      appliedScale: 1.5,
      rect: { x: -1280, y: -100, width: 630, height: 780 },
    }, async () => { throw saveError; })).rejects.toBe(saveError);

    expect(preferences).toEqual(old);
  });
});
