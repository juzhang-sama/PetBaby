import { decide, type PetStateSnapshot } from "../behavior/decision";
import {
  initialCatMotionSchedulerState,
  scheduleCatMotion,
  type CatMotionSchedulerState,
} from "../behavior/cat-motion-scheduler";
import type { CatMotionEvent, PetEvent } from "../behavior/events";
import {
  isPetCalibrationPreviewRequest,
  isPetCalibrationPreviewResult,
  type PetCalibrationPreviewAction,
  type PetCalibrationPreviewRequest,
  type PetCalibrationPreviewResult,
} from "./contracts";
import {
  canonicalPetCalibration,
  DEFAULT_PET_CALIBRATION,
  type PetCalibrationV1,
} from "./pet-calibration";
import type { PetPresentationPorts } from "./pet-presentation-controller";
import { PetPresentationController } from "./pet-presentation-controller";
import type { PetRenderer } from "./pet-renderer";
import { RenderScheduler } from "./render-scheduler";
import type { WindowPoint } from "./window-motion-controller";

interface StageEventTarget {
  addEventListener(type: string, listener: EventListener): void;
  removeEventListener(type: string, listener: EventListener): void;
}

export type StageWindowMotion = PetPresentationPorts["windowMotion"] & {
  beginDrag(pointer: WindowPoint, dpr?: number): Promise<void>;
  dragTo(pointer: WindowPoint): Promise<void>;
  endDrag(): Promise<void>;
  update(deltaMs: number): Promise<void>;
};

export type StageEffectOverlay = PetPresentationPorts["effects"] & { destroy(): void };

export interface PetStageOptions {
  renderer: PetRenderer;
  windowMotion: StageWindowMotion;
  effects: StageEffectOverlay;
  pointerTarget?: StageEventTarget;
  resizeTarget?: StageEventTarget;
  requestFrame?: (callback: FrameRequestCallback) => number;
  cancelFrame?: (handle: number) => void;
  devicePixelRatio?: () => number;
  setTimer?: (callback: () => void, delayMs: number) => number;
  clearTimer?: (handle: number) => void;
  refreshHitRegion?: () => Promise<void>;
  diagnose?: (stage: string, error: unknown) => void;
  random?: () => number;
  localHour?: () => number;
  onFrameSample?: (deltas: readonly number[]) => void;
}

const DRAG_THRESHOLD_PX = 4;

export class PetStage {
  private readonly renderer: PetRenderer;
  private readonly windowMotion: StageWindowMotion;
  private readonly effects: StageEffectOverlay;
  private readonly pointerTarget: StageEventTarget;
  private readonly resizeTarget: StageEventTarget;
  private readonly requestFrame: (callback: FrameRequestCallback) => number;
  private readonly cancelFrame: (handle: number) => void;
  private readonly devicePixelRatio: () => number;
  private readonly setTimer: (callback: () => void, delayMs: number) => number;
  private readonly clearTimer: (handle: number) => void;
  private readonly scheduler: RenderScheduler;
  private readonly presentation: PetPresentationController;
  private root: HTMLElement | null = null;
  private running = false;
  private destroyed = false;
  private frameHandle: number | null = null;
  private maxFps = 24;
  private lastFrameAt: number | null = null;
  private nextFrameAt: number | null = null;
  private activeTimer: number | null = null;
  private pointerDown: WindowPoint | null = null;
  private dragging = false;
  private visible = false;
  private windowModeTransitionPaused = false;
  private catMotionState: CatMotionSchedulerState = initialCatMotionSchedulerState();
  private catMotionEnabled = false;
  private readonly random: () => number;
  private readonly localHour: () => number;
  private frameSampleDeltas: number[] = [];
  private frameSampleElapsedMs = 0;

  private readonly stateSnapshot: PetStateSnapshot = {
    schemaVersion: 1,
    petId: "runtime",
    energy: 0.7,
    mood: 0.6,
    bond: 0.3,
    lastSeenAt: new Date().toISOString(),
    lastInteractionAt: new Date().toISOString(),
  };

  constructor(private readonly options: PetStageOptions) {
    this.renderer = options.renderer;
    this.windowMotion = options.windowMotion;
    this.effects = options.effects;
    this.pointerTarget = options.pointerTarget
      ?? (typeof document === "undefined" ? unavailableEventTarget() : document);
    this.resizeTarget = options.resizeTarget
      ?? (typeof window === "undefined" ? this.pointerTarget : window);
    this.requestFrame = options.requestFrame
      ?? ((callback) => window.requestAnimationFrame(callback));
    this.cancelFrame = options.cancelFrame
      ?? ((handle) => window.cancelAnimationFrame(handle));
    this.devicePixelRatio = options.devicePixelRatio
      ?? (() => typeof window === "undefined" ? 1 : window.devicePixelRatio || 1);
    this.setTimer = options.setTimer
      ?? ((callback, delayMs) => window.setTimeout(callback, delayMs));
    this.clearTimer = options.clearTimer
      ?? ((handle) => window.clearTimeout(handle));
    this.random = options.random ?? Math.random;
    this.localHour = options.localHour ?? (() => new Date().getHours());
    this.scheduler = new RenderScheduler({
      start: () => this.startFrames(),
      stop: () => this.stopFrames(),
      setMaxFps: (fps) => { this.maxFps = fps; },
      renderOnce: () => this.renderFrame(0),
    });
    this.presentation = new PetPresentationController({
      renderer: this.renderer,
      effects: this.effects,
      windowMotion: this.windowMotion,
      scheduler: this.scheduler,
    });
  }

  async mount(root: HTMLElement): Promise<void> {
    if (this.destroyed) throw new Error("PetStage has been destroyed");
    this.root = root;
    this.resize();
    this.renderer.setVisibility(true);
    this.visible = true;
    this.catMotionEnabled = this.isCatV4();
    this.scheduler.setCompanionFps(this.catMotionEnabled ? 60 : 24);
    if (this.catMotionEnabled) this.dispatchCatEvent({ type: "start" });
    else this.renderer.playMotion("idle", { priority: 10, loop: true });
    this.renderFrame(0);
    root.addEventListener("pointerdown", this.onPointerDown);
    root.addEventListener("pointerenter", this.onPointerEnter);
    root.addEventListener("pointerleave", this.onPointerLeave);
    root.addEventListener("dblclick", this.onDoubleClick);
    this.pointerTarget.addEventListener("pointermove", this.onPointerMove);
    this.pointerTarget.addEventListener("pointerup", this.onPointerUp);
    this.pointerTarget.addEventListener("pointercancel", this.onPointerCancel);
    this.resizeTarget.addEventListener("resize", this.onResize);
    this.scheduler.setTier("companion");
    await this.refreshHitRegion();
  }

  setVisibility(visible: boolean): void {
    if (this.destroyed) return;
    if (this.windowModeTransitionPaused) {
      return;
    }
    this.applyVisibility(visible);
  }

  pauseWindowModeTransition(): void {
    if (this.destroyed || this.windowModeTransitionPaused) return;
    this.windowModeTransitionPaused = true;
    this.applyVisibility(false);
  }

  resumeWindowModeTransition(effectiveVisible: boolean): void {
    if (this.destroyed || !this.windowModeTransitionPaused) return;
    this.windowModeTransitionPaused = false;
    this.applyVisibility(effectiveVisible);
  }

  abortWindowModeTransition(): void {
    if (this.destroyed) return;
    this.windowModeTransitionPaused = true;
    this.applyVisibility(false);
  }

  private applyVisibility(visible: boolean): void {
    this.visible = visible;
    if (visible) {
      this.renderer.setVisibility(true);
      if (this.catMotionEnabled) this.dispatchCatEvent({ type: "start" });
      else this.presentation.dispatch({ type: "awake" });
      this.scheduler.setTier("companion");
      return;
    }
    if (this.catMotionEnabled) this.suspendCatMotion();
    else this.presentation.dispatch({ type: "sleep" });
    this.renderer.setVisibility(false);
    this.scheduler.setTier("paused");
  }

  refreshViewport(): void {
    if (this.destroyed) return;
    this.resize();
  }

  setCalibration(value: PetCalibrationV1): void {
    if (this.destroyed) return;
    const calibration = canonicalPetCalibration(value);
    this.renderer.setCalibration(calibration);
    this.presentation.setCalibration(calibration);
  }

  previewFeedback(): void {
    if (this.destroyed) return;
    this.presentation.dispatch({ type: "react-curious" });
  }

  syncActiveRenderer(): void {
    if (this.destroyed) return;
    const nextCatMotionEnabled = this.isCatV4();
    if (!nextCatMotionEnabled && !this.catMotionEnabled) return;
    this.presentation.cancelCatMotions();
    this.catMotionState = initialCatMotionSchedulerState();
    this.catMotionEnabled = nextCatMotionEnabled;
    this.scheduler.setCompanionFps(this.catMotionEnabled ? 60 : 24);
    if (!this.visible) return;
    if (this.catMotionEnabled) this.dispatchCatEvent({ type: "start" });
    else this.renderer.playMotion("idle", { priority: 10, loop: true });
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.scheduler.setTier("paused");
    if (this.activeTimer !== null) this.clearTimer(this.activeTimer);
    this.root?.removeEventListener("pointerdown", this.onPointerDown);
    this.root?.removeEventListener("pointerenter", this.onPointerEnter);
    this.root?.removeEventListener("pointerleave", this.onPointerLeave);
    this.root?.removeEventListener("dblclick", this.onDoubleClick);
    this.pointerTarget.removeEventListener("pointermove", this.onPointerMove);
    this.pointerTarget.removeEventListener("pointerup", this.onPointerUp);
    this.pointerTarget.removeEventListener("pointercancel", this.onPointerCancel);
    this.resizeTarget.removeEventListener("resize", this.onResize);
    this.renderer.setVisibility(false);
    this.renderer.destroy();
    this.effects.destroy();
    this.root = null;
  }

  private readonly onPointerDown = (event: Event): void => {
    const pointer = event as PointerEvent;
    if (pointer.button !== 0) return;
    this.pointerDown = { x: pointer.screenX, y: pointer.screenY };
    this.dragging = false;
    this.scheduler.setTier("active");
  };

  private readonly onPointerEnter = (): void => {
    if (this.catMotionEnabled && !this.pointerDown) this.dispatchCatEvent({ type: "pointer-enter" });
  };

  private readonly onPointerLeave = (): void => {
    if (this.catMotionEnabled && !this.pointerDown) {
      this.renderer.setLookTarget(null);
      this.dispatchCatEvent({ type: "pointer-leave" });
    }
  };

  private readonly onPointerMove = (event: Event): void => {
    const pointer = event as PointerEvent;
    if (this.catMotionEnabled && !this.pointerDown && this.root) {
      const bounds = this.root.getBoundingClientRect();
      const x = ((pointer.clientX - bounds.left) / Math.max(1, this.root.clientWidth)) * 2 - 1;
      const y = 1 - ((pointer.clientY - bounds.top) / Math.max(1, this.root.clientHeight)) * 2;
      this.renderer.setLookTarget({
        x: Math.min(1, Math.max(-1, x)),
        y: Math.min(1, Math.max(-1, y)),
      });
      return;
    }
    void this.movePointer({ x: pointer.screenX, y: pointer.screenY });
  };

  private readonly onPointerUp = (event: Event): void => {
    const pointer = event as PointerEvent;
    void this.releasePointer({ x: pointer.clientX, y: pointer.clientY });
  };

  private readonly onPointerCancel = (): void => {
    void this.releasePointer(null);
  };

  private readonly onDoubleClick = (): void => {
    if (this.catMotionEnabled) this.dispatchCatEvent({ type: "pet" });
    else this.dispatchEvent({ type: "double-clicked" });
  };

  private readonly onResize = (): void => {
    this.refreshViewport();
    void this.refreshHitRegion();
  };

  private async movePointer(pointer: WindowPoint): Promise<void> {
    if (!this.pointerDown) return;
    if (!this.dragging) {
      const distance = Math.hypot(pointer.x - this.pointerDown.x, pointer.y - this.pointerDown.y);
      if (distance < DRAG_THRESHOLD_PX) return;
      this.dragging = true;
      await this.windowMotion.beginDrag(this.pointerDown, this.devicePixelRatio());
      if (this.catMotionEnabled) {
        this.renderer.setLookTarget(null);
        this.dispatchCatEvent({ type: "drag-start" });
      }
      else this.dispatchEvent({ type: "drag-start" });
    }
    await this.windowMotion.dragTo(pointer);
  }

  private async releasePointer(pointer: WindowPoint | null): Promise<void> {
    if (!this.pointerDown) return;
    const wasDragging = this.dragging;
    this.pointerDown = null;
    this.dragging = false;
    if (wasDragging) {
      await this.windowMotion.endDrag();
      if (this.catMotionEnabled) this.dispatchCatEvent({ type: "drag-end" });
      else this.dispatchEvent({ type: "drag-end" });
    } else if (pointer && this.root) {
      const bounds = this.root.getBoundingClientRect();
      const area = this.renderer.hitTest({ x: pointer.x - bounds.left, y: pointer.y - bounds.top });
      if (area) {
        if (this.catMotionEnabled) this.dispatchCatEvent({ type: "pet" });
        else this.dispatchEvent({ type: area === "head" ? "head-clicked" : "body-clicked" });
      }
    }
    if (this.activeTimer !== null) this.clearTimer(this.activeTimer);
    this.activeTimer = this.setTimer(() => this.scheduler.setTier("companion"), 400);
  }

  private dispatchEvent(event: PetEvent): void {
    const intents = decide({ event, state: this.stateSnapshot, policy: { cooldowns: {} } });
    for (const intent of intents) this.presentation.dispatch(intent);
  }

  private dispatchCatEvent(event: CatMotionEvent): void {
    if (
      event.type === "pet"
      || event.type === "drag-start"
      || event.type === "edge-hidden"
      || event.type === "edge-recall"
    ) {
      this.renderer.setLookTarget(null);
    }
    const result = scheduleCatMotion(this.catMotionState, event, {
      localHour: this.localHour(),
      random: this.random,
      paused: !this.visible || this.windowModeTransitionPaused,
    });
    this.catMotionState = result.state;
    this.renderer.setCatAutomationMode?.(automationModeFor(this.catMotionState.mode));
    this.presentation.dispatchCatMotion(result.commands, (token) => {
      if (this.destroyed || !this.catMotionEnabled) return;
      this.dispatchCatEvent({ type: "motion-complete", token });
    });
  }

  private isCatV4(): boolean {
    return this.renderer.supportsCatMotionV1?.() === true;
  }

  private suspendCatMotion(): void {
    this.presentation.cancelCatMotions();
    this.renderer.setCatAutomationMode?.("paused");
    this.catMotionState = {
      ...this.catMotionState,
      mode: "idle",
      activeToken: null,
      activePriority: 0,
    };
  }

  private resize(): void {
    if (!this.root) return;
    this.renderer.resize({
      width: Math.max(1, this.root.clientWidth),
      height: Math.max(1, this.root.clientHeight),
      dpr: Math.max(1, this.devicePixelRatio()),
    });
  }

  private async refreshHitRegion(): Promise<void> {
    try {
      await this.options.refreshHitRegion?.();
    } catch (error) {
      this.options.diagnose?.("hit-region", error);
    }
  }

  private startFrames(): void {
    if (this.running || this.destroyed) return;
    this.running = true;
    this.frameHandle = this.requestFrame(this.onFrame);
  }

  private stopFrames(): void {
    this.running = false;
    this.lastFrameAt = null;
    this.nextFrameAt = null;
    this.frameSampleDeltas = [];
    this.frameSampleElapsedMs = 0;
    if (this.frameHandle !== null) this.cancelFrame(this.frameHandle);
    this.frameHandle = null;
  }

  private readonly onFrame = (now: number): void => {
    if (!this.running || this.destroyed) return;
    const minimumFrameMs = 1_000 / this.maxFps;
    if (this.lastFrameAt === null || this.nextFrameAt === null) {
      this.lastFrameAt = now;
      this.nextFrameAt = now + minimumFrameMs;
      this.renderFrame(0);
    } else if (now + 0.5 >= this.nextFrameAt) {
      const deltaMs = Math.max(0, now - this.lastFrameAt);
      this.lastFrameAt = now;
      do {
        this.nextFrameAt += minimumFrameMs;
      } while (this.nextFrameAt <= now + 0.5);
      this.renderFrame(deltaMs);
    }
    this.frameHandle = this.requestFrame(this.onFrame);
  };

  private renderFrame(deltaMs: number): void {
    if (this.catMotionEnabled && deltaMs > 0) this.dispatchCatEvent({ type: "tick", elapsedMs: deltaMs });
    this.renderer.update(deltaMs);
    void this.windowMotion.update(deltaMs).catch((error) => this.options.diagnose?.("window-motion", error));
    if (deltaMs <= 0 || !this.options.onFrameSample) return;
    this.frameSampleDeltas.push(deltaMs);
    this.frameSampleElapsedMs += deltaMs;
    if (this.frameSampleElapsedMs < 1_000) return;
    this.options.onFrameSample(this.frameSampleDeltas);
    this.frameSampleDeltas = [];
    this.frameSampleElapsedMs = 0;
  }
}

function automationModeFor(
  mode: CatMotionSchedulerState["mode"],
): "idle" | "pointerFocus" | "dragging" | "paused" {
  if (mode === "pointerFocus") return "pointerFocus";
  if (mode === "dragging") return "dragging";
  if (mode === "idle" || mode === "edgeHidden") return "idle";
  return "paused";
}

export interface PetCalibrationPreviewClientPorts {
  listen(handler: (result: unknown) => void): Promise<() => void>;
  emit(request: PetCalibrationPreviewRequest): Promise<void>;
}

export interface RequestPetCalibrationPreviewOptions {
  ports: PetCalibrationPreviewClientPorts;
  requestIdFactory?: () => string;
  timeoutMs?: number;
}

const activeCalibrationRequestIds = new Set<string>();

export function requestPetCalibrationPreview(
  petId: string,
  action: PetCalibrationPreviewAction,
  value: PetCalibrationV1,
  options: RequestPetCalibrationPreviewOptions,
): Promise<PetCalibrationPreviewResult> {
  const requestId = (options.requestIdFactory ?? (() => crypto.randomUUID()))();
  let canonical: PetCalibrationV1;
  try {
    canonical = canonicalPetCalibration(value);
  } catch (error) {
    return Promise.reject(error);
  }
  const request: PetCalibrationPreviewRequest = { requestId, petId, action, value: canonical };
  if (!isPetCalibrationPreviewRequest(request)) {
    return Promise.reject(new TypeError("Invalid pet calibration preview request"));
  }
  const timeoutMs = options.timeoutMs ?? 5_000;
  if (!Number.isFinite(timeoutMs) || timeoutMs < 0) {
    return Promise.reject(new RangeError("timeoutMs must be a finite non-negative number"));
  }
  if (activeCalibrationRequestIds.has(requestId)) {
    return Promise.reject(new Error(`Pet calibration request id is already active: ${requestId}`));
  }
  activeCalibrationRequestIds.add(requestId);

  return new Promise<PetCalibrationPreviewResult>((resolve, reject) => {
    let settled = false;
    let unlisten: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const cleanup = (): void => {
      activeCalibrationRequestIds.delete(requestId);
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      const dispose = unlisten;
      unlisten = undefined;
      try { dispose?.(); } catch { /* Cleanup cannot alter the request outcome. */ }
    };
    const rejectOnce = (error: unknown): void => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error instanceof Error ? error : new Error(String(error)));
    };
    const resolveOnce = (result: PetCalibrationPreviewResult): void => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(result);
    };
    timer = setTimeout(
      () => rejectOnce(new Error(`Pet calibration preview request timed out after ${timeoutMs / 1_000} seconds`)),
      timeoutMs,
    );
    void (async () => {
      try {
        const dispose = await options.ports.listen((candidate) => {
          if (!isPetCalibrationPreviewResult(candidate)
            || candidate.requestId !== requestId
            || candidate.petId !== petId
            || candidate.action !== action) return;
          resolveOnce(candidate);
        });
        if (settled) {
          try { dispose(); } catch { /* A late listener cannot reopen a settled request. */ }
          return;
        }
        unlisten = dispose;
        await options.ports.emit(request);
      } catch (error) {
        rejectOnce(error);
      }
    })();
  });
}

export interface PetCalibrationPreviewListenerOptions {
  listen(handler: (request: unknown) => void): Promise<() => void>;
  emit(result: PetCalibrationPreviewResult): Promise<void>;
  activePetId(): string;
  savedCalibration(): PetCalibrationV1;
  setCalibration(value: PetCalibrationV1): void;
  commitSaved(petId: string, value: PetCalibrationV1): void;
  previewFeedback(): void;
  diagnose?(stage: string, error: unknown): void;
}

export async function listenForPetCalibrationPreviewRequests(
  options: PetCalibrationPreviewListenerOptions,
): Promise<() => void> {
  let destroyed = false;
  const pending = new Map<string, PetCalibrationPreviewRequest>();
  const completed = new Map<string, { request: PetCalibrationPreviewRequest; result: PetCalibrationPreviewResult }>();
  let queue = Promise.resolve();
  const diagnose = (stage: string, error: unknown): void => {
    try { options.diagnose?.(stage, error); } catch { /* Diagnostics are observational. */ }
  };
  const emit = async (result: PetCalibrationPreviewResult): Promise<void> => {
    if (destroyed) return;
    try { await options.emit(result); } catch (error) { diagnose("calibration-preview-result", error); }
  };
  const process = async (request: PetCalibrationPreviewRequest): Promise<void> => {
    if (destroyed) return;
    let result: PetCalibrationPreviewResult;
    try {
      if (options.activePetId() !== request.petId) throw new Error("Calibration target is not the active pet");
      const applied = request.action === "restore"
        ? canonicalPetCalibration(options.savedCalibration())
        : canonicalPetCalibration(request.value);
      if (request.action === "commit") options.commitSaved(request.petId, applied);
      else options.setCalibration(applied);
      if (request.action === "feedback") options.previewFeedback();
      result = { ...request, value: applied, ok: true };
    } catch (error) {
      result = {
        requestId: request.requestId,
        petId: request.petId,
        action: request.action,
        ok: false,
        message: errorMessage(error).slice(0, 2_048) || "Calibration preview failed",
      };
    }
    pending.delete(request.requestId);
    completed.set(request.requestId, { request, result });
    if (completed.size > 128) completed.delete(completed.keys().next().value as string);
    await emit(result);
  };
  const receive = (candidate: unknown): void => {
    if (destroyed) return;
    if (!isPetCalibrationPreviewRequest(candidate)) {
      diagnose("calibration-preview-request", new TypeError("Invalid pet calibration preview request"));
      return;
    }
    const previous = completed.get(candidate.requestId);
    if (previous) {
      const same = JSON.stringify(previous.request) === JSON.stringify(candidate);
      queue = queue.then(() => emit(same ? previous.result : calibrationConflict(candidate)));
      return;
    }
    const inFlight = pending.get(candidate.requestId);
    if (inFlight) {
      const same = JSON.stringify(inFlight) === JSON.stringify(candidate);
      queue = queue.then(async () => {
        const finished = completed.get(candidate.requestId);
        await emit(same && finished ? finished.result : calibrationConflict(candidate));
      });
      return;
    }
    pending.set(candidate.requestId, candidate);
    queue = queue.then(() => process(candidate));
  };
  const unlisten = await options.listen(receive);
  return () => {
    if (destroyed) return;
    destroyed = true;
    pending.clear();
    completed.clear();
    try { unlisten(); } catch { /* Teardown is best-effort and idempotent. */ }
  };
}

function calibrationConflict(request: PetCalibrationPreviewRequest): PetCalibrationPreviewResult {
  return {
    requestId: request.requestId,
    petId: request.petId,
    action: request.action,
    ok: false,
    message: "requestId is already bound to another calibration preview request",
  };
}

export interface PetCalibrationRuntimeOptions {
  activePetId(): string;
  load(petId: string): Promise<unknown>;
  setCalibration(value: PetCalibrationV1): void;
  diagnose?(stage: string, error: unknown): void;
}

export class PetCalibrationRuntime {
  private saved: PetCalibrationV1 = { ...DEFAULT_PET_CALIBRATION };
  private revision = 0;

  constructor(private readonly options: PetCalibrationRuntimeOptions) {}

  savedCalibration(): PetCalibrationV1 {
    return { ...this.saved };
  }

  commitSaved(petId: string, value: PetCalibrationV1): void {
    const canonical = canonicalPetCalibration(value);
    if (this.options.activePetId() !== petId) {
      throw new Error("Calibration can only be committed for the active pet");
    }
    this.options.setCalibration(canonical);
    this.saved = canonical;
  }

  async activate(petId: string): Promise<void> {
    const revision = ++this.revision;
    if (this.options.activePetId() !== petId) return;
    const fallback = canonicalPetCalibration(DEFAULT_PET_CALIBRATION);
    this.options.setCalibration(fallback);
    this.saved = fallback;
    try {
      const loaded = canonicalPetCalibration(await this.options.load(petId));
      if (!this.isCurrent(revision, petId)) return;
      this.options.setCalibration(loaded);
      this.saved = loaded;
    } catch (error) {
      if (!this.isCurrent(revision, petId)) return;
      try { this.options.diagnose?.("calibration-load", error); } catch { /* Diagnostics are observational. */ }
    }
  }

  private isCurrent(revision: number, petId: string): boolean {
    return this.revision === revision && this.options.activePetId() === petId;
  }
}

export interface PetCalibrationWiringOptions extends PetCalibrationRuntimeOptions {
  listen(handler: (request: unknown) => void): Promise<() => void>;
  emit(result: PetCalibrationPreviewResult): Promise<void>;
  previewFeedback(): void;
}

export interface PetCalibrationWiring {
  afterPetSwitch(result: { ok: boolean; petId: string }): Promise<void>;
  destroy(): void;
}

export async function wirePetCalibrationRuntime(
  options: PetCalibrationWiringOptions,
): Promise<PetCalibrationWiring> {
  const runtime = new PetCalibrationRuntime(options);
  await runtime.activate(options.activePetId());
  const unlisten = await listenForPetCalibrationPreviewRequests({
    listen: options.listen,
    emit: options.emit,
    activePetId: options.activePetId,
    savedCalibration: () => runtime.savedCalibration(),
    setCalibration: options.setCalibration,
    commitSaved: (petId, value) => runtime.commitSaved(petId, value),
    previewFeedback: options.previewFeedback,
    diagnose: options.diagnose,
  });
  let destroyed = false;
  return {
    afterPetSwitch: async (result) => {
      if (destroyed || !result.ok) return;
      await runtime.activate(result.petId);
    },
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      unlisten();
    },
  };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function unavailableEventTarget(): StageEventTarget {
  return { addEventListener() {}, removeEventListener() {} };
}
