import { decide, type PetStateSnapshot } from "../behavior/decision";
import type { PetEvent } from "../behavior/events";
import type { PetEffect, PetPresentationPorts } from "./pet-presentation-controller";
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

export interface StageEffectOverlay {
  play(effect: PetEffect): void;
  destroy(): void;
}

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
  private activeTimer: number | null = null;
  private pointerDown: WindowPoint | null = null;
  private dragging = false;

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
    this.renderer.playMotion("idle", { priority: 10, loop: true });
    this.renderFrame(0);
    root.addEventListener("pointerdown", this.onPointerDown);
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
    if (visible) {
      this.renderer.setVisibility(true);
      this.presentation.dispatch({ type: "awake" });
      this.scheduler.setTier("companion");
      return;
    }
    this.presentation.dispatch({ type: "sleep" });
    this.renderer.setVisibility(false);
    this.scheduler.setTier("paused");
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.scheduler.setTier("paused");
    if (this.activeTimer !== null) this.clearTimer(this.activeTimer);
    this.root?.removeEventListener("pointerdown", this.onPointerDown);
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

  private readonly onPointerMove = (event: Event): void => {
    const pointer = event as PointerEvent;
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
    this.dispatchEvent({ type: "double-clicked" });
  };

  private readonly onResize = (): void => {
    this.resize();
    void this.refreshHitRegion();
  };

  private async movePointer(pointer: WindowPoint): Promise<void> {
    if (!this.pointerDown) return;
    if (!this.dragging) {
      const distance = Math.hypot(pointer.x - this.pointerDown.x, pointer.y - this.pointerDown.y);
      if (distance < DRAG_THRESHOLD_PX) return;
      this.dragging = true;
      await this.windowMotion.beginDrag(this.pointerDown, this.devicePixelRatio());
      this.dispatchEvent({ type: "drag-start" });
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
      this.dispatchEvent({ type: "drag-end" });
    } else if (pointer && this.root) {
      const bounds = this.root.getBoundingClientRect();
      const area = this.renderer.hitTest({ x: pointer.x - bounds.left, y: pointer.y - bounds.top });
      if (area) this.dispatchEvent({ type: area === "head" ? "head-clicked" : "body-clicked" });
    }
    if (this.activeTimer !== null) this.clearTimer(this.activeTimer);
    this.activeTimer = this.setTimer(() => this.scheduler.setTier("companion"), 400);
  }

  private dispatchEvent(event: PetEvent): void {
    const intents = decide({ event, state: this.stateSnapshot, policy: { cooldowns: {} } });
    for (const intent of intents) this.presentation.dispatch(intent);
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
    if (this.frameHandle !== null) this.cancelFrame(this.frameHandle);
    this.frameHandle = null;
  }

  private readonly onFrame = (now: number): void => {
    if (!this.running || this.destroyed) return;
    const deltaMs = this.lastFrameAt === null ? Math.max(0, now) : Math.max(0, now - this.lastFrameAt);
    const minimumFrameMs = 1_000 / this.maxFps;
    if (this.lastFrameAt === null || deltaMs >= minimumFrameMs) {
      this.lastFrameAt = now;
      this.renderFrame(deltaMs);
    }
    this.frameHandle = this.requestFrame(this.onFrame);
  };

  private renderFrame(deltaMs: number): void {
    this.renderer.update(deltaMs);
    void this.windowMotion.update(deltaMs).catch((error) => this.options.diagnose?.("window-motion", error));
  }
}

function unavailableEventTarget(): StageEventTarget {
  return { addEventListener() {}, removeEventListener() {} };
}
