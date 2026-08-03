import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { availableMonitors, getCurrentWindow, primaryMonitor } from "@tauri-apps/api/window";
import { Application, Assets, Sprite } from "pixi.js";
import { applyHitRegion, beginDrag, loadPreferences, savePreferences } from "./bridge";
import type { ProbePreferences } from "./contracts";
import { clampRectToWorkArea, computeContainRect } from "./geometry";
import { alphaToRegionSpans } from "./hit-mask";
import { RenderScheduler } from "./render-scheduler";

export class PetStage {
  private readonly app = new Application();
  private sprite!: Sprite;
  private source!: CanvasImageSource;
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

  async mount(root: HTMLElement): Promise<void> {
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
    const texture = await Assets.load("/test-assets/pet-probe.png");
    this.source = texture.source.resource as CanvasImageSource;
    this.sprite = new Sprite(texture);
    this.app.stage.addChild(this.sprite);
    await this.layoutAndApplyRegion();
    this.scheduler.setTier("still");

    this.app.canvas.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      this.scheduler.setTier("active");
      window.clearTimeout(this.activeTimer);
      void beginDrag().then(() => this.captureWindowPlacement());
    });
    this.app.canvas.addEventListener("pointerup", () => {
      window.clearTimeout(this.activeTimer);
      this.activeTimer = window.setTimeout(() => this.scheduler.setTier("still"), 400);
    });
    this.app.canvas.addEventListener("wheel", (event) => {
      event.preventDefault();
      const delta = event.deltaY < 0 ? 0.1 : -0.1;
      void this.setScale(Math.min(1.5, Math.max(0.5, this.preferences.scale + delta)));
    }, { passive: false });
    this.app.canvas.addEventListener("dblclick", () => {
      void this.setFlipped(!this.preferences.flipped);
    });
    window.addEventListener("resize", () => void this.layoutAndApplyRegion());
  }

  private async layoutAndApplyRegion(): Promise<void> {
    const layout = computeContainRect(
      { width: this.sprite.texture.width, height: this.sprite.texture.height },
      { width: this.root.clientWidth, height: this.root.clientHeight },
    );
    this.baseScale = layout.scale;
    this.sprite.anchor.set(0.5, 0);
    this.sprite.position.set(this.root.clientWidth / 2, layout.y);
    this.sprite.scale.set(
      (this.preferences.flipped ? -1 : 1) * this.baseScale * this.preferences.scale,
      this.baseScale * this.preferences.scale,
    );
    this.app.render();
    await this.updateHitRegion();
  }

  private async updateHitRegion(): Promise<void> {
    const canvas = document.createElement("canvas");
    canvas.width = this.root.clientWidth;
    canvas.height = this.root.clientHeight;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("2D canvas is unavailable for hit-mask extraction");
    const width = this.sprite.texture.width * this.baseScale * this.preferences.scale;
    const height = this.sprite.texture.height * this.baseScale * this.preferences.scale;
    const x = (canvas.width - width) / 2;
    const y = this.sprite.y;
    context.save();
    context.translate(this.preferences.flipped ? canvas.width : 0, 0);
    context.scale(this.preferences.flipped ? -1 : 1, 1);
    context.drawImage(this.source, x, y, width, height);
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
    const intersects = monitors.some((monitor) => {
      const area = { x: monitor.position.x, y: monitor.position.y, width: monitor.size.width, height: monitor.size.height };
      return saved.x < area.x + area.width && saved.x + saved.width > area.x
        && saved.y < area.y + area.height && saved.y + saved.height > area.y;
    });
    let restored = saved;
    if (!intersects) {
      const monitor = await primaryMonitor();
      if (monitor) {
        restored = clampRectToWorkArea(saved, {
          x: monitor.position.x,
          y: monitor.position.y,
          width: monitor.size.width,
          height: monitor.size.height,
        });
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
