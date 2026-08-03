import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { availableMonitors, getCurrentWindow, primaryMonitor } from "@tauri-apps/api/window";
import { Application, Assets, Graphics, Text } from "pixi.js";
import { applyHitRegion, loadPreferences, savePreferences } from "./bridge";
import type { ProbePreferences } from "./contracts";
import { clampRectToWorkArea, computeContainRect } from "./geometry";
import { alphaToRegionSpans } from "./hit-mask";
import { LayeredSprite } from "../assets/layered-sprite";
import { decide } from "../behavior/decision";
import type { PetEvent } from "../behavior/events";
import type { PetStateSnapshot } from "../behavior/decision";
import { PetAnimator } from "./pet-animator";
import { RenderScheduler } from "./render-scheduler";

export class PetStage {
  private readonly app = new Application();
  private layered!: LayeredSprite;
  private bodySource!: CanvasImageSource;
  private root!: HTMLElement;
  private preferences!: ProbePreferences;
  private baseScale = 1;
  private saveTimer: number | undefined;
  private activeTimer: number | undefined;
  private readonly scheduler = new RenderScheduler({
    start: () => this.app.start(),
    stop: () => this.app.stop(),
    setMaxFps: (fps) => { this.app.ticker.maxFPS = fps; },
    renderOnce: () => this.app.render(),
  });
  private animator!: PetAnimator;
  private stateSnapshot: PetStateSnapshot = {
    schemaVersion: 1,
    petId: "probe",
    energy: 0.7,
    mood: 0.6,
    bond: 0.3,
    lastSeenAt: new Date().toISOString(),
    lastInteractionAt: new Date().toISOString(),
  };

  async mount(root: HTMLElement, degraded?: { status: string }): Promise<void> {
    this.root = root;
    this.preferences = await loadPreferences();
    const petWindow = getCurrentWindow();
    await this.restoreWindowPlacement();

    await this.app.init({
      resizeTo: root,
      backgroundAlpha: 0,
      antialias: true,
      autoStart: false,
      preference: "webgl",
    });
    root.replaceChildren(this.app.canvas);
    if (degraded) {
      this.renderDegradedPlaceholder(degraded.status);
      this.scheduler.setTier("still");
      return;
    }
    this.layered = new LayeredSprite({
      bodyUrl: "/test-assets/layered/body.png",
      eyeOpenUrl: "/test-assets/layered/eye-open.png",
      eyeClosedUrl: "/test-assets/layered/eye-closed.png",
      accentUrl: "/test-assets/layered/accent.png",
    });
    await this.layered.mount(
      this.app.stage,
      { width: this.root.clientWidth, height: this.root.clientHeight },
      this.preferences.flipped,
    );
    const bodyTexture = await Assets.load("/test-assets/layered/body.png");
    this.bodySource = bodyTexture.source.resource as CanvasImageSource;
    this.animator = new PetAnimator({
      setEyesOpen: (open) => this.layered.setEyesOpen(open),
      setBreathPhase: (phase) => this.layered.setBreathPhase(phase),
      scaleSquash: (factor) => this.layered.setSquash(factor),
      shift: (dx, dy) => this.layered.setShift(dx, dy),
      setAccentVisible: (visible) => this.layered.setAccentVisible(visible),
    });
    await this.layoutAndApplyRegion();
    this.animator.start();
    this.scheduler.setTier("companion");

    this.app.ticker.add(() => {
      this.animator.tick(this.app.ticker.lastTime);
    });

    this.app.canvas.addEventListener("pointerdown", (event) => {
      void this.onPointerDown(event);
    });
    document.addEventListener("pointermove", (event) => {
      void this.onPointerMove(event);
    });
    document.addEventListener("pointerup", () => {
      void this.onPointerUp();
    });
    document.addEventListener("pointercancel", () => {
      void this.onPointerUp();
    });
    this.app.canvas.addEventListener("wheel", (event) => {
      event.preventDefault();
      const delta = event.deltaY < 0 ? 0.1 : -0.1;
      void this.setScale(Math.min(1.5, Math.max(0.5, this.preferences.scale + delta)));
    }, { passive: false });
    this.app.canvas.addEventListener("dblclick", () => {
      this.dispatchEvent({ type: "double-clicked" });
      void this.setFlipped(!this.preferences.flipped);
    });
    window.addEventListener("resize", () => void this.layoutAndApplyRegion());
  }

  private async layoutAndApplyRegion(): Promise<void> {
    const bodyTexture = await Assets.load("/test-assets/layered/body.png");
    const layout = computeContainRect(
      { width: bodyTexture.width, height: bodyTexture.height },
      { width: this.root.clientWidth, height: this.root.clientHeight },
    );
    this.baseScale = layout.scale;
    this.layered.setFlip(this.preferences.flipped);
    this.layered.setUserScale(this.preferences.scale);
    this.app.render();
    await this.updateHitRegion();
  }

  private dragging = false;
  private dragStartMouse: { x: number; y: number } | null = null;
  private dragStartWindow: { x: number; y: number } | null = null;

  private renderDegradedPlaceholder(status: string): void {
    const width = this.root.clientWidth;
    const height = this.root.clientHeight;
    const box = new Graphics()
      .roundRect(width / 2 - 90, height / 2 - 70, 180, 140, 24)
      .fill({ color: 0x88909a, alpha: 0.9 });
    const label = new Text({
      text: status === "missing" ? "资产缺失" : "资产损坏",
      style: { fill: 0xffffff, fontSize: 20, fontWeight: "600" },
    });
    label.anchor.set(0.5, 0.5);
    label.position.set(width / 2, height / 2);
    const hint = new Text({
      text: "请在设置中重新导入",
      style: { fill: 0xe8e8e8, fontSize: 12 },
    });
    hint.anchor.set(0.5, 0.5);
    hint.position.set(width / 2, height / 2 + 34);
    this.app.stage.addChild(box, label, hint);
    this.app.render();
  }

  private async onPointerDown(event: PointerEvent): Promise<void> {
    if (event.button !== 0) return;
    this.scheduler.setTier("active");
    window.clearTimeout(this.activeTimer);
    this.dispatchEvent({ type: "drag-start" });
    const petWindow = getCurrentWindow();
    const [position] = await Promise.all([petWindow.outerPosition()]);
    this.dragging = true;
    this.dragStartMouse = { x: event.screenX, y: event.screenY };
    this.dragStartWindow = { x: position.x, y: position.y };
  }

  private async onPointerMove(event: PointerEvent): Promise<void> {
    if (!this.dragging || !this.dragStartMouse || !this.dragStartWindow) return;
    const dpr = window.devicePixelRatio || 1;
    const dx = (event.screenX - this.dragStartMouse.x) * dpr;
    const dy = (event.screenY - this.dragStartMouse.y) * dpr;
    const petWindow = getCurrentWindow();
    await petWindow.setPosition(new PhysicalPosition(
      Math.round(this.dragStartWindow.x + dx),
      Math.round(this.dragStartWindow.y + dy),
    ));
  }

  private async onPointerUp(): Promise<void> {
    const wasDragging = this.dragging;
    this.dragging = false;
    this.dragStartMouse = null;
    this.dragStartWindow = null;
    window.clearTimeout(this.activeTimer);
    this.dispatchEvent(wasDragging ? { type: "drag-end" } : { type: "body-clicked" });
    this.activeTimer = window.setTimeout(() => this.scheduler.setTier("companion"), 400);
    if (wasDragging) await this.captureWindowPlacement();
  }

  private dispatchEvent(event: PetEvent): void {
    const intents = decide({ event, state: this.stateSnapshot, policy: { cooldowns: {} } });
    for (const intent of intents) {
      this.animator.setIntent(intent);
    }
  }

  private async updateHitRegion(): Promise<void> {
    const canvas = document.createElement("canvas");
    canvas.width = this.root.clientWidth;
    canvas.height = this.root.clientHeight;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("2D canvas is unavailable for hit-mask extraction");
    const bodyTexture = await Assets.load("/test-assets/layered/body.png");
    const width = bodyTexture.width * this.baseScale * this.preferences.scale;
    const height = bodyTexture.height * this.baseScale * this.preferences.scale;
    const x = (canvas.width - width) / 2;
    const y = canvas.height - height;
    context.save();
    context.translate(this.preferences.flipped ? canvas.width : 0, 0);
    context.scale(this.preferences.flipped ? -1 : 1, 1);
    context.drawImage(this.bodySource, x, y, width, height);
    context.restore();
    const image = context.getImageData(0, 0, canvas.width, canvas.height);
    await applyHitRegion({
      canvasWidth: canvas.width,
      canvasHeight: canvas.height,
      scaleFactor: window.devicePixelRatio,
      spans: alphaToRegionSpans(image.data, image.width, image.height, { alphaThreshold: 32, rowStep: 2 }),
    });
  }

  private async setScale(scale: number): Promise<void> {
    this.preferences.scale = scale;
    await this.layoutAndApplyRegion();
    this.scheduleSave();
  }

  private async setFlipped(flipped: boolean): Promise<void> {
    this.preferences.flipped = flipped;
    await this.layoutAndApplyRegion();
    this.scheduleSave();
  }

  private async captureWindowPlacement(): Promise<void> {
    const petWindow = getCurrentWindow();
    const [position, size] = await Promise.all([petWindow.outerPosition(), petWindow.outerSize()]);
    Object.assign(this.preferences, {
      x: position.x,
      y: position.y,
      width: size.width,
      height: size.height,
    });
    this.scheduleSave();
  }

  private async restoreWindowPlacement(): Promise<void> {
    const petWindow = getCurrentWindow();
    const monitors = await availableMonitors();
    const saved = {
      x: this.preferences.x,
      y: this.preferences.y,
      width: this.preferences.width,
      height: this.preferences.height,
    };
    const defaultSize = { width: 420, height: 520 };

    let restored = saved;
    let anchored = false;
    for (const monitor of monitors) {
      const area = { x: monitor.position.x, y: monitor.position.y, width: monitor.size.width, height: monitor.size.height };
      const overlaps = saved.x < area.x + area.width && saved.x + saved.width > area.x
        && saved.y < area.y + area.height && saved.y + saved.height > area.y;
      if (!overlaps) continue;
      // size larger than 95% of a monitor is treated as a leftover snap state
      if (saved.width > area.width * 0.95 || saved.height > area.height * 0.95) {
        restored = { ...clampRectToWorkArea(saved, area, 64), ...defaultSize };
      } else {
        restored = clampRectToWorkArea(saved, area, 64);
      }
      anchored = true;
      break;
    }
    if (!anchored) {
      const monitor = await primaryMonitor();
      if (monitor) {
        const area = { x: monitor.position.x, y: monitor.position.y, width: monitor.size.width, height: monitor.size.height };
        if (saved.width > area.width * 0.95 || saved.height > area.height * 0.95) {
          restored = { ...clampRectToWorkArea(saved, area, 64), ...defaultSize };
        } else {
          restored = clampRectToWorkArea(saved, area, 64);
        }
      }
    }
    Object.assign(this.preferences, restored);
    await petWindow.setSize(new PhysicalSize(restored.width, restored.height));
    await petWindow.setPosition(new PhysicalPosition(restored.x, restored.y));
  }

  private scheduleSave(): void {
    window.clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => void savePreferences(this.preferences), 250);
  }
}
