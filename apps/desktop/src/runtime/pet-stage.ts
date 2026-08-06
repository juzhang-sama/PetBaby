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
import { evolveState, type PetAction } from "../behavior/state";
import { markCooldown, type PolicySnapshot } from "../behavior/policy";
import type { ManifestMeshFeatures, ManifestPart } from "./manifest-schema";
import { PetAnimator } from "./pet-animator";
import { ParticleSystem } from "./particles";
import {
  DEFAULT_PHYSICS_CONFIG,
  chaseStep,
  edgeStrollStep,
  stepPhysics,
  throwPet,
  type PetPhysicsState,
  type PhysicsBounds,
} from "./pet-physics";
import { RenderScheduler } from "./render-scheduler";

export class PetStage {
  // pet size is fixed: wheel zoom was removed by product decision; the subject
  // is laid out to fill the window, and the default size is half of that
  private static readonly FIXED_USER_SCALE = 0.5;
  private static readonly THROW_SPEED_PX_S = 60;
  private static readonly STROLL_DURATION_MS = 2_800;
  private static readonly CHASE_DURATION_MS = 4_000;
  private static readonly TASKBAR_MARGIN_PX = 72;
  private static readonly EVENT_COOLDOWNS: Record<string, { key: string; ms: number }> = {
    "double-clicked": { key: "react-happy", ms: 5_000 },
    "drag-start": { key: "carried", ms: 2_000 },
    petted: { key: "pet", ms: 4_000 },
    fed: { key: "feed", ms: 10_000 },
    played: { key: "play", ms: 12_000 },
  };

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
  private assetUrls: { bodyUrl: string } = { bodyUrl: "/test-assets/layered/body.png" };
  private stateSnapshot: PetStateSnapshot = {
    schemaVersion: 1,
    petId: "probe",
    energy: 0.7,
    mood: 0.6,
    bond: 0.3,
    lastSeenAt: new Date().toISOString(),
    lastInteractionAt: new Date().toISOString(),
  };
  private readonly policy: PolicySnapshot = { cooldowns: {} };
  private readonly particles = new ParticleSystem();
  private particleGfx!: Graphics;
  private sleeping = false;
  private lastFrameAt = 0;
  private stateElapsedMs = 0;
  private idleElapsedMs = 0;
  private idleTickCount = 0;
  private interactionMenu: HTMLDivElement | null = null;
  private physics: PetPhysicsState = { x: 0, y: 0, vx: 0, vy: 0, mode: "idle", direction: 1 };
  private physicsBounds: PhysicsBounds | null = null;
  private readonly dragSamples: Array<{ t: number; x: number; y: number }> = [];
  private lastCursor = { x: 0, y: 0 };
  private strollElapsedMs = 0;
  private chaseUntil = 0;

  async mount(
    root: HTMLElement,
    degraded?: { status: string },
    assets?: {
      bodyUrl: string;
      eyeOpenUrl: string;
      eyeClosedUrl: string;
      accentUrl?: string;
      parts?: ManifestPart[];
      meshFeatures?: ManifestMeshFeatures;
    },
  ): Promise<void> {
    this.root = root;
    this.preferences = await loadPreferences();
    const petWindow = getCurrentWindow();
    await this.restoreWindowPlacement();
    const monitor = await primaryMonitor();
    const dpr = window.devicePixelRatio || 1;
    if (monitor) {
      const winWidth = window.innerWidth * dpr;
      const winHeight = window.innerHeight * dpr;
      this.physicsBounds = {
        left: monitor.position.x,
        top: monitor.position.y,
        right: monitor.position.x + monitor.size.width - winWidth,
        bottom: monitor.position.y + monitor.size.height - winHeight - PetStage.TASKBAR_MARGIN_PX,
      };
      this.physics = { ...this.physics, x: this.preferences.x, y: this.preferences.y };
    }
    if (assets) this.assetUrls = { bodyUrl: assets.bodyUrl };

    await this.app.init({
      backgroundAlpha: 0,
      antialias: true,
      autoStart: false,
      preference: "webgl",
    });
    root.replaceChildren(this.app.canvas);
    this.resizeRendererToViewport();
    if (degraded) {
      this.renderDegradedPlaceholder(degraded.status, this.currentViewport());
      this.scheduler.setTier("still");
      return;
    }
    this.layered = new LayeredSprite(
      assets ?? {
        bodyUrl: "/test-assets/layered/body.png",
        eyeOpenUrl: "/test-assets/layered/eye-open.png",
        eyeClosedUrl: "/test-assets/layered/eye-closed.png",
        accentUrl: "/test-assets/layered/accent.png",
      },
      assets?.parts ?? [],
      assets?.meshFeatures,
    );
    await this.layered.mount(
      this.app.stage,
      this.currentViewport(),
      this.preferences.flipped,
    );
    this.particleGfx = new Graphics();
    this.app.stage.addChild(this.particleGfx);
    const bodyTexture = await Assets.load(this.assetUrls.bodyUrl);
    this.bodySource = bodyTexture.source.resource as CanvasImageSource;
    this.animator = new PetAnimator({
      setEyesOpen: (open) => this.layered.setEyesOpen(open),
      setBreathPhase: (phase) => this.layered.setBreathPhase(phase),
      scaleSquash: (factor) => this.layered.setSquash(factor),
      shift: (dx, dy) => this.layered.setShift(dx, dy),
      setAccentVisible: (visible) => this.layered.setAccentVisible(visible),
      setTilt: (angle) => this.layered.setTilt(angle),
      setHeadTurn: (amount) => this.layered.setHeadTurn(amount),
    });
    await this.layoutAndApplyRegion();
    this.animator.start();
    this.scheduler.setTier("companion");

    this.app.ticker.add(() => this.onFrame(this.app.ticker.lastTime));

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
    this.app.canvas.addEventListener("dblclick", () => {
      this.dispatchEvent({ type: "double-clicked" });
    });
    this.app.canvas.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      this.showInteractionMenu(event.clientX, event.clientY);
    });
    document.addEventListener("pointerdown", (event) => {
      const target = event.target as HTMLElement | null;
      if (this.interactionMenu && target && !this.interactionMenu.contains(target)) {
        this.hideInteractionMenu();
      }
    });
    window.addEventListener("resize", () => {
      this.resizeRendererToViewport();
      this.layered.relayout(this.currentViewport());
      void this.layoutAndApplyRegion();
    });

  }

  private currentViewport(): { width: number; height: number } {
    return {
      width: window.innerWidth || this.root?.clientWidth || 420,
      height: window.innerHeight || this.root?.clientHeight || 520,
    };
  }

  private resizeRendererToViewport(): void {
    const viewport = this.currentViewport();
    this.app.renderer.resize(viewport.width, viewport.height);
    this.app.canvas.style.width = `${viewport.width}px`;
    this.app.canvas.style.height = `${viewport.height}px`;
  }

  private async layoutAndApplyRegion(): Promise<void> {
    this.layered.setFlip(this.preferences.flipped);
    this.layered.setUserScale(PetStage.FIXED_USER_SCALE);
    this.app.render();
    await this.updateHitRegion();
  }

  private dragging = false;
  private dragStartMouse: { x: number; y: number } | null = null;
  private dragStartWindow: { x: number; y: number } | null = null;

  private renderDegradedPlaceholder(status: string, viewport: { width: number; height: number }): void {
    const width = viewport.width;
    const height = viewport.height;
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
    this.physics = { ...this.physics, mode: "idle", vx: 0, vy: 0 };
    this.dragSamples.length = 0;
    this.dispatchEvent({ type: "drag-start" });
    const petWindow = getCurrentWindow();
    const [position] = await Promise.all([petWindow.outerPosition()]);
    this.dragging = true;
    this.dragStartMouse = { x: event.screenX, y: event.screenY };
    this.dragStartWindow = { x: position.x, y: position.y };
  }

  private async onPointerMove(event: PointerEvent): Promise<void> {
    this.lastCursor = { x: event.screenX, y: event.screenY };
    if (!this.dragging || !this.dragStartMouse || !this.dragStartWindow) return;
    this.dragSamples.push({ t: performance.now(), x: event.screenX, y: event.screenY });
    while (this.dragSamples.length > 8) this.dragSamples.shift();
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
    const dpr = window.devicePixelRatio || 1;
    let velocity: { x: number; y: number } | undefined;
    if (wasDragging && this.dragSamples.length >= 2) {
      const a = this.dragSamples[this.dragSamples.length - 2]!;
      const b = this.dragSamples[this.dragSamples.length - 1]!;
      const dtMs = Math.max(16, b.t - a.t);
      velocity = {
        x: ((b.x - a.x) / dtMs) * 1_000 * dpr,
        y: ((b.y - a.y) / dtMs) * 1_000 * dpr,
      };
    }
    this.dragSamples.length = 0;
    const speed = velocity ? Math.hypot(velocity.x, velocity.y) : 0;
    if (wasDragging && speed >= PetStage.THROW_SPEED_PX_S && velocity) {
      this.physics = throwPet(this.physics, velocity.x, velocity.y);
      this.dispatchEvent({ type: "drag-end", velocity });
    } else {
      this.dispatchEvent(wasDragging ? { type: "drag-end" } : { type: "body-clicked" });
    }
    this.activeTimer = window.setTimeout(() => this.scheduler.setTier("companion"), 400);
    if (wasDragging) await this.captureWindowPlacement();
  }

  private dispatchEvent(event: PetEvent): void {
    const intents = decide({ event, state: this.stateSnapshot, policy: this.policy });
    const cooldown = PetStage.EVENT_COOLDOWNS[event.type];
    if (cooldown && intents.length > 0) {
      markCooldown(this.policy, cooldown.key, Date.now(), cooldown.ms);
    }
    for (const intent of intents) {
      this.animator.setIntent(intent);
      this.feedbackFor(intent);
    }
  }

  private feedbackFor(intent: { type: string }): void {
    if (!this.layered) return;
    const bounds = this.layered.getBodyBounds();
    const centerX = bounds.x + bounds.width / 2;
    if (intent.type === "react-happy") {
      this.particles.spawn("heart", centerX, bounds.y + bounds.height * 0.35, { count: 3 });
    } else if (intent.type === "react-curious") {
      this.particles.spawn("spark", centerX, bounds.y + bounds.height * 0.25, { count: 2 });
    } else if (intent.type === "sleep") {
      this.particles.spawn("zzz", centerX + bounds.width * 0.25, bounds.y + bounds.height * 0.15, { count: 1 });
    }
  }

  private onFrame(now: number): void {
    const dt = this.lastFrameAt === 0 ? 16 : Math.min(now - this.lastFrameAt, 250);
    this.lastFrameAt = now;
    this.animator.tick(now);
    this.particles.update(dt);
    this.drawParticles();
    this.stepPhysicsFrame(dt, now);

    this.stateElapsedMs += dt;
    this.idleElapsedMs += dt;
    if (this.stateElapsedMs >= 60_000) {
      this.stateSnapshot = evolveState(this.stateSnapshot, new Date(), this.stateElapsedMs);
      this.stateElapsedMs = 0;
      if (this.stateSnapshot.energy < 0.2 && !this.sleeping) {
        this.sleeping = true;
        this.animator.setIntent({ type: "sleep" });
        this.feedbackFor({ type: "sleep" });
      } else if (this.stateSnapshot.energy >= 0.3 && this.sleeping) {
        this.sleeping = false;
        this.animator.setIntent({ type: "awake" });
      }
    }
    if (this.idleElapsedMs >= 15_000) {
      this.idleTickCount += 1;
      this.maybeStartIdleAction();
      this.dispatchEvent({ type: "idle-tick", elapsedMs: this.idleElapsedMs });
      this.idleElapsedMs = 0;
    }
  }

  private stepPhysicsFrame(dt: number, now: number): void {
    const bounds = this.physicsBounds;
    if (!bounds) return;
    if (this.sleeping && this.physics.mode !== "idle") {
      this.physics = { ...this.physics, mode: "idle", vx: 0, vy: 0 };
      this.animator.setMode("idle");
      return;
    }
    if (this.physics.mode === "idle") {
      this.strollElapsedMs = 0;
      return;
    }
    let next: PetPhysicsState;
    if (this.physics.mode === "falling") {
      next = stepPhysics(this.physics, dt, bounds, DEFAULT_PHYSICS_CONFIG);
    } else if (this.physics.mode === "strolling") {
      this.strollElapsedMs += dt;
      next = this.strollElapsedMs >= PetStage.STROLL_DURATION_MS
        ? { ...this.physics, mode: "idle", vx: 0, vy: 0 }
        : edgeStrollStep(this.physics, dt, bounds, DEFAULT_PHYSICS_CONFIG);
    } else {
      next = now >= this.chaseUntil
        ? { ...this.physics, mode: "idle", vx: 0, vy: 0 }
        : chaseStep(
          this.physics,
          dt,
          this.lastCursor.x * (window.devicePixelRatio || 1),
          bounds,
          DEFAULT_PHYSICS_CONFIG,
        );
    }
    const settled = next.mode === "idle";
    this.physics = next;
    void this.applyPhysicsPosition();
    if (settled) {
      this.animator.setMode("idle");
      this.dispatchEvent({ type: "landed" });
      void this.captureWindowPlacement();
    }
  }

  private async applyPhysicsPosition(): Promise<void> {
    const petWindow = getCurrentWindow();
    await petWindow.setPosition(
      new PhysicalPosition(Math.round(this.physics.x), Math.round(this.physics.y)),
    );
  }

  private maybeStartIdleAction(): void {
    if (this.sleeping || this.physics.mode !== "idle" || !this.physicsBounds) return;
    if (this.idleTickCount % 3 !== 0) return;
    const bounds = this.physicsBounds;
    const nearEdge = this.physics.x <= bounds.left + 160
      || this.physics.x >= bounds.right - 160;
    if (nearEdge) {
      this.startStroll();
    } else if (this.idleTickCount % 6 === 0) {
      this.startChase();
    }
  }

  private startStroll(): void {
    const bounds = this.physicsBounds;
    if (!bounds) return;
    const direction: 1 | -1 = this.physics.x <= bounds.left + 160 ? 1 : -1;
    this.physics = {
      ...this.physics,
      y: bounds.bottom,
      direction,
      vx: 0,
      vy: 0,
      mode: "strolling",
    };
    this.strollElapsedMs = 0;
    this.animator.setIntent({ type: "stroll" });
  }

  private startChase(): void {
    const bounds = this.physicsBounds;
    if (!bounds) return;
    const dpr = window.devicePixelRatio || 1;
    const targetX = this.lastCursor.x * dpr;
    const direction: 1 | -1 = targetX >= this.physics.x ? 1 : -1;
    this.chaseUntil = performance.now() + PetStage.CHASE_DURATION_MS;
    this.physics = {
      ...this.physics,
      y: bounds.bottom,
      direction,
      vx: 0,
      vy: 0,
      mode: "chasing",
    };
    this.animator.setIntent({ type: "stroll" });
  }

  private drawParticles(): void {
    if (!this.particleGfx) return;
    const gfx = this.particleGfx;
    gfx.clear();
    for (const particle of this.particles.active) {
      const alpha = Math.max(0, Math.min(1, particle.life / particle.maxLife));
      if (particle.kind === "heart") {
        const size = particle.size;
        gfx
          .circle(particle.x - size / 2, particle.y, size / 2)
          .circle(particle.x + size / 2, particle.y, size / 2)
          .poly([particle.x - size, particle.y, particle.x + size, particle.y, particle.x, particle.y + size * 1.2])
          .fill({ color: 0xff6b81, alpha });
      } else if (particle.kind === "spark") {
        gfx.circle(particle.x, particle.y, particle.size * 0.45).fill({ color: 0xffd166, alpha });
      } else {
        const s = particle.size;
        gfx
          .moveTo(particle.x - s, particle.y)
          .lineTo(particle.x + s, particle.y)
          .moveTo(particle.x + s, particle.y)
          .lineTo(particle.x - s, particle.y + s)
          .moveTo(particle.x - s, particle.y + s)
          .lineTo(particle.x + s, particle.y + s)
          .stroke({ color: 0x93c5fd, width: 2, alpha });
      }
    }
  }

  private dispatchInteraction(event: PetEvent, action: PetAction): void {
    this.stateSnapshot = evolveState(this.stateSnapshot, new Date(), 0, action);
    this.dispatchEvent(event);
    this.hideInteractionMenu();
  }

  private ensureInteractionMenu(): HTMLDivElement {
    if (this.interactionMenu) return this.interactionMenu;
    const menu = document.createElement("div");
    menu.style.cssText = [
      "position:fixed",
      "z-index:99",
      "background:#fff",
      "border:1px solid #fed7aa",
      "border-radius:12px",
      "box-shadow:0 6px 20px rgba(0,0,0,.15)",
      "padding:6px",
      "display:flex",
      "flex-direction:column",
      "gap:4px",
      "display:none",
    ].join(";");
    const entries: Array<[string, PetEvent, PetAction]> = [
      ["抚摸", { type: "petted" }, "pet"],
      ["喂食", { type: "fed" }, "feed"],
      ["玩耍", { type: "played" }, "play"],
    ];
    for (const [label, event, action] of entries) {
      const button = document.createElement("button");
      button.textContent = label;
      button.style.cssText = [
        "border:none",
        "background:#fffaf3",
        "border-radius:8px",
        "padding:6px 16px",
        "font-size:13px",
        "cursor:pointer",
        "color:#431407",
      ].join(";");
      button.addEventListener("click", () => this.dispatchInteraction(event, action));
      menu.append(button);
    }
    document.body.append(menu);
    this.interactionMenu = menu;
    return menu;
  }

  private showInteractionMenu(x: number, y: number): void {
    const menu = this.ensureInteractionMenu();
    const left = Math.max(4, Math.min(x, window.innerWidth - 96));
    const top = Math.max(4, Math.min(y, window.innerHeight - 130));
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
    menu.style.display = "flex";
  }

  private hideInteractionMenu(): void {
    if (this.interactionMenu) this.interactionMenu.style.display = "none";
  }

  private async updateHitRegion(): Promise<void> {
    const canvas = document.createElement("canvas");
    const viewport = this.currentViewport();
    canvas.width = viewport.width;
    canvas.height = viewport.height;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("2D canvas is unavailable for hit-mask extraction");
    const bodyTexture = await Assets.load(this.assetUrls.bodyUrl);
    const bounds = this.layered.getBodyBounds();
    context.save();
    context.translate(this.preferences.flipped ? canvas.width : 0, 0);
    context.scale(this.preferences.flipped ? -1 : 1, 1);
    context.drawImage(this.bodySource, bounds.x, bounds.y, bounds.width, bounds.height);
    context.restore();
    const image = context.getImageData(0, 0, canvas.width, canvas.height);
    await applyHitRegion({
      canvasWidth: canvas.width,
      canvasHeight: canvas.height,
      scaleFactor: window.devicePixelRatio,
      spans: alphaToRegionSpans(image.data, image.width, image.height, { alphaThreshold: 32, rowStep: 2 }),
    });
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
    const fullVisiblePixels = Math.max(saved.width, saved.height);
    for (const monitor of monitors) {
      const area = { x: monitor.position.x, y: monitor.position.y, width: monitor.size.width, height: monitor.size.height };
      const overlaps = saved.x < area.x + area.width && saved.x + saved.width > area.x
        && saved.y < area.y + area.height && saved.y + saved.height > area.y;
      if (!overlaps) continue;
      // size larger than 95% of a monitor is treated as a leftover snap state
            if (saved.width > area.width * 0.95 || saved.height > area.height * 0.95) {
                restored = { ...clampRectToWorkArea(saved, area, fullVisiblePixels), ...defaultSize };
            } else {
                restored = clampRectToWorkArea(saved, area, fullVisiblePixels);
            }
      anchored = true;
      break;
    }
    if (!anchored) {
      const monitor = await primaryMonitor();
      if (monitor) {
        const area = { x: monitor.position.x, y: monitor.position.y, width: monitor.size.width, height: monitor.size.height };
                if (saved.width > area.width * 0.95 || saved.height > area.height * 0.95) {
                    restored = { ...clampRectToWorkArea(saved, area, fullVisiblePixels), ...defaultSize };
                } else {
                    restored = clampRectToWorkArea(saved, area, fullVisiblePixels);
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
