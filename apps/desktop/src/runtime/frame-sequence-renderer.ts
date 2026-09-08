import type { FrameSequenceIdleSchedule, NormalizedRectV6 } from "./frame-sequence-manifest";
import { computeContainRect, type LayoutRect } from "./geometry";
import type {
  PetCalibrationV1,
} from "./pet-calibration";
import type {
  PetExpression,
  PetHitArea,
  PetMotion,
  PetMotionHandle,
  PetRenderAsset,
  PetRenderer,
  FrameSequenceRenderAction,
} from "./pet-renderer";
import { loadBrowserImage } from "./static-png-renderer";

type FrameImage = CanvasImageSource & { width: number; height: number };

interface Viewport {
  width: number;
  height: number;
  dpr: number;
}

export interface FrameSequenceRendererOptions {
  createCanvas?: () => HTMLCanvasElement;
  /**
   * Factory for the offscreen surface used to bake a per-action union silhouette.
   * Defaults to `document.createElement("canvas")`; hosts without a DOM (unit
   * tests) leave it undefined and fall back to the base pose.
   */
  createMaskCanvas?: () => HTMLCanvasElement;
  loadImage?: (url: string) => Promise<FrameImage>;
  random?: () => number;
}

interface LoadedAction {
  action: FrameSequenceRenderAction;
  frames: FrameImage[];
}

export class FrameSequenceRenderer implements PetRenderer {
  private readonly displayCanvas: HTMLCanvasElement;
  private readonly hitCanvas: HTMLCanvasElement;
  private readonly displayContext: CanvasRenderingContext2D;
  private readonly hitContext: CanvasRenderingContext2D;
  private readonly loadImage: (url: string) => Promise<FrameImage>;
  private readonly createMaskCanvas: (() => HTMLCanvasElement) | undefined;
  private readonly random: () => number;

  private baseImage: FrameImage | undefined;
  private actions: LoadedAction[] = [];
  private defaultAction: string | undefined;
  private semantics: Record<string, string> = {};
  private idleSchedule: FrameSequenceIdleSchedule | null = null;
  private hitBounds: NormalizedRectV6 | undefined;
  // 后台加载（非默认动作）的完成信号，测试与调用方可 await 等待全部帧就绪。
  private backgroundLoad: Promise<void> = Promise.resolve();

  private viewport: Viewport | undefined;
  private bounds: LayoutRect | undefined;
  private visible = false;
  private destroyed = false;
  private loadToken = 0;

  // 所有动作的定义级元信息（不依赖帧是否已加载）。holdRange 决定交互保持窗口。
  private actionDefs = new Map<
    string,
    { frameDurationMs: number; holdRange?: readonly [number, number] }
  >();
  // 交互保持态：playMotion(<有 holdRange 的动作>, { loop: true }) 进入——
  // 播完"拎起"段后在 holdRange 窗口内循环悬空，直到同一动作以非 loop 再触发（松手）。
  private holdHeld = false;

  private currentActionId: string | undefined;
  private actionElapsedMs = 0;
  // 窗口裁剪区（Windows SetWindowRgn）跟随 hit surface。动作切换时 hit surface 会
  // 换成该动作的"帧并集"轮廓，这里记录当前画的是哪个动作 + 待刷新标记。
  private renderedHitActionId: string | undefined;
  private silhouetteDirty = false;
  private unionMasks = new Map<string, FrameImage>();
  private actionGeneration = 0;
  private actionLoop = false;
  private idleAccumulatedMs = 0;
  private nextOneShotAtMs = Infinity;
  private lastOneShotActionId: string | null = null;
  private lastOneShotAtMs = 0;
  // Set when an idleSchedule trigger fired but the default loop has not yet
  // reached its boundary (alignToDefaultLoop mode).
  private pendingAlignedOneShot = false;

  constructor(
    private readonly root: HTMLElement,
    options: FrameSequenceRendererOptions = {},
  ) {
    const createCanvas = options.createCanvas ?? (() => document.createElement("canvas"));
    this.displayCanvas = createCanvas();
    this.hitCanvas = createCanvas();
    const displayContext = this.displayCanvas.getContext("2d");
    const hitContext = this.hitCanvas.getContext("2d");
    if (!displayContext || !hitContext) {
      throw new Error("2D canvas is unavailable for frame sequence rendering");
    }
    this.displayContext = displayContext;
    this.hitContext = hitContext;
    this.displayContext.imageSmoothingEnabled = false;
    this.hitContext.imageSmoothingEnabled = false;
    this.loadImage = options.loadImage ?? loadBrowserImage;
    this.createMaskCanvas = options.createMaskCanvas
      ?? (typeof document === "undefined" ? undefined : () => document.createElement("canvas"));
    this.random = options.random ?? Math.random;
    this.displayCanvas.style.display = "block";
    this.hitCanvas.style.display = "none";
    this.displayCanvas.style.visibility = "hidden";
    this.hitCanvas.style.visibility = "hidden";
  }

  async load(asset: PetRenderAsset): Promise<void> {
    this.assertAlive();
    if (asset.kind !== "frame-sequence") {
      throw new TypeError("FrameSequenceRenderer only accepts frame-sequence assets");
    }
    const loadToken = ++this.loadToken;
    try {
      const baseImage = await this.loadImage(asset.baseImageUrl);
      if (this.destroyed || loadToken !== this.loadToken) return;

      // 默认动作阻塞加载（首帧显示必需）；其余动作后台有界并发加载，
      // 避免启动时一次性解码 500+ 帧（内存峰值 ~726MB）并串行拖慢首帧。
      const defaultActionDef = asset.actions.find((a) => a.actionId === asset.defaultAction);
      const eager: LoadedAction[] = [];
      const deferred: FrameSequenceRenderAction[] = [];
      for (const action of asset.actions) {
        if (!defaultActionDef || action.actionId === asset.defaultAction) {
          const frames: FrameImage[] = [];
          for (const url of action.frameUrls) {
            frames.push(await this.loadImage(url));
          }
          eager.push({ action, frames });
        } else {
          deferred.push(action);
        }
      }
      if (this.destroyed || loadToken !== this.loadToken) return;

      this.baseImage = baseImage;
      this.actions = eager;
      this.defaultAction = asset.defaultAction;
      this.actionDefs = new Map(
        asset.actions.map((a) => [a.actionId, { frameDurationMs: a.frameDurationMs, holdRange: a.holdRange }]),
      );
      this.holdHeld = false;
      this.semantics = { ...asset.semantics };
      this.idleSchedule = asset.idleSchedule;
      this.hitBounds = asset.hitBounds;
      this.currentActionId = asset.defaultAction;
      this.actionElapsedMs = 0;
      this.unionMasks = new Map();
      this.renderedHitActionId = undefined;
      this.silhouetteDirty = false;
      // 默认动作是常驻循环（呼吸），必须按 manifest 的 loop 标志初始化。
      // 若初始化为 false，第一个循环会被当成一次性动作，在周期边界重置
      // actionElapsedMs，导致 alignToDefaultLoop 的相位计算错位
      // （触发点对不上真实的循环边界）。
      const defaultLoaded = this.actions.find((a) => a.action.actionId === asset.defaultAction);
      this.actionLoop = defaultLoaded?.action.loop ?? true;
      this.idleAccumulatedMs = 0;
      this.nextOneShotAtMs = this.rollOneShotDelay();
      this.lastOneShotActionId = null;
      this.lastOneShotAtMs = 0;
      this.pendingAlignedOneShot = false;
      this.root.replaceChildren(this.displayCanvas, this.hitCanvas);
      this.recomputeLayout();
      this.renderDisplay();

      // 非默认动作（yawn/lick 等偶发动作）后台加载，不阻塞首帧。
      this.backgroundLoad = this.loadDeferred(deferred, loadToken);
    } catch (error) {
      if (this.destroyed || loadToken !== this.loadToken) return;
      throw error;
    }
  }

  /**
   * 后台有界并发加载非默认动作。加载完成后逐个追加到 this.actions——
   * idleSchedule 的 actionExists 门会自动把它们纳入偶发动作候选，未加载前不会被触发。
   * 全程 loadToken 守卫：新 load() 或 destroy() 使本批次作废。
   */
  private async loadDeferred(
    actions: FrameSequenceRenderAction[],
    loadToken: number,
  ): Promise<void> {
    const CONCURRENCY = 6;
    const queue = [...actions];
    const worker = async (): Promise<void> => {
      while (queue.length > 0) {
        if (this.destroyed || loadToken !== this.loadToken) return;
        const action = queue.shift()!;
        const frames: FrameImage[] = [];
        for (const url of action.frameUrls) {
          if (this.destroyed || loadToken !== this.loadToken) return;
          frames.push(await this.loadImage(url));
        }
        if (this.destroyed || loadToken !== this.loadToken) return;
        this.actions.push({ action, frames });
      }
    };
    await Promise.all(
      Array.from({ length: Math.min(CONCURRENCY, Math.max(1, queue.length)) }, () => worker()),
    );
  }

  /** 等待后台加载的非默认动作全部就绪（测试与调用方专用）。 */
  async whenReady(): Promise<void> {
    await this.backgroundLoad;
  }

  resize(viewport: Viewport): void {
    this.assertAlive();
    if (viewport.width <= 0 || viewport.height <= 0 || viewport.dpr <= 0) {
      throw new RangeError("viewport dimensions and dpr must be positive");
    }
    this.viewport = { ...viewport };
    for (const canvas of [this.displayCanvas, this.hitCanvas]) {
      canvas.width = Math.max(1, Math.round(viewport.width * viewport.dpr));
      canvas.height = Math.max(1, Math.round(viewport.height * viewport.dpr));
      canvas.style.width = `${viewport.width}px`;
      canvas.style.height = `${viewport.height}px`;
    }
    this.displayContext.setTransform(viewport.dpr, 0, 0, viewport.dpr, 0, 0);
    this.hitContext.setTransform(viewport.dpr, 0, 0, viewport.dpr, 0, 0);
    this.displayContext.imageSmoothingEnabled = false;
    this.hitContext.imageSmoothingEnabled = false;
    // 视口变了，已画好的 hit surface 坐标失效，强制下一次 renderDisplay 重画。
    this.renderedHitActionId = undefined;
    this.recomputeLayout();
    this.renderDisplay();
  }

  playMotion(motion: PetMotion, options?: { loop?: boolean; priority?: number }): PetMotionHandle {
    if (this.destroyed || !this.defaultAction) return { cancel: () => undefined };
    const actionId = this.semantics[motion] ?? this.defaultAction;
    const generation = ++this.actionGeneration;
    // 松手语义：正持握着该动作（悬空窗口循环中），又来一发不带 loop 的同动作
    // playMotion（landed）→ 不重置进度，把进度快照到 hold 窗口末尾，续播"放下+稳定"尾段。
    // 若在进入 hold 窗口前就松手（lift 中途），则保留当前进度线性播完（经过悬空帧自然下落）。
    const releasing = this.holdHeld && this.currentActionId === actionId
      && !(options?.loop ?? false);
    this.holdHeld = false;
    this.currentActionId = actionId;
    if (!releasing) {
      this.actionElapsedMs = 0;
    } else {
      const def = this.actionDefs.get(actionId);
      const holdRange = def?.holdRange;
      const duration = def?.frameDurationMs ?? 0;
      if (holdRange && duration > 0 && this.actionElapsedMs >= holdRange[0] * duration) {
        this.actionElapsedMs = (holdRange[1] + 1) * duration;
      }
    }
    this.actionLoop = options?.loop ?? (motion === "idle");
    // loop + 动作声明 holdRange → 交互保持：播放到区间后只在窗口内循环悬空。
    if (this.actionLoop && this.actionDefs.get(actionId)?.holdRange) {
      this.holdHeld = true;
    }
    this.idleAccumulatedMs = 0;
    this.pendingAlignedOneShot = false;
    let active = true;
    return {
      cancel: () => {
        if (!active) return;
        active = false;
        if (generation === this.actionGeneration && this.defaultAction) {
          this.currentActionId = this.defaultAction;
          this.actionElapsedMs = 0;
          this.actionLoop = false;
          this.holdHeld = false;
          this.idleAccumulatedMs = 0;
          this.pendingAlignedOneShot = false;
        }
      },
    };
  }

  setExpression(_expression: PetExpression, _weight?: number): void {}

  setLookTarget(_target: { x: number; y: number } | null): void {}

  setLipSync(_value: number): void {}

  setCalibration(_value: PetCalibrationV1): void {}

  hitTest(point: { x: number; y: number }): PetHitArea | null {
    if (!this.baseImage || !this.viewport || !this.bounds || !this.visible || this.destroyed) return null;
    const hit = this.hitBounds ?? { left: 0, top: 0, right: 1, bottom: 1 };
    const left = this.bounds.x + hit.left * this.bounds.width;
    const top = this.bounds.y + hit.top * this.bounds.height;
    const right = this.bounds.x + hit.right * this.bounds.width;
    const bottom = this.bounds.y + hit.bottom * this.bounds.height;
    return point.x >= left && point.x < right && point.y >= top && point.y < bottom ? "body" : null;
  }

  setVisibility(visible: boolean): void {
    if (this.destroyed) return;
    this.visible = visible;
    const visibility = visible ? "visible" : "hidden";
    this.displayCanvas.style.visibility = visibility;
    this.hitCanvas.style.visibility = visibility;
  }

  update(deltaMs: number): void {
    if (!this.visible || this.destroyed || !Number.isFinite(deltaMs) || deltaMs < 0) return;
    if (deltaMs === 0) return;
    this.actionElapsedMs += deltaMs;
    if (!this.currentActionId || !this.defaultAction) return;

    const loaded = this.findAction(this.currentActionId);
    if (loaded) {
      const duration = loaded.action.frameDurationMs * loaded.frames.length;
      if (!this.actionLoop && this.actionElapsedMs >= duration) {
        this.lastOneShotActionId = this.currentActionId;
        this.lastOneShotAtMs = this.idleAccumulatedMs;
        this.currentActionId = this.defaultAction;
        // Resume the default loop at phase 0: one-shot frames built on the
        // default action's phase-0 body (composited blink) end exactly on that
        // pose, so this hand-off is pixel-seamless. The default loop pausing
        // for the one-shot duration is the accepted plan-B trade-off.
        this.actionElapsedMs = 0;
        this.actionLoop = true;
        this.idleAccumulatedMs = 0;
        this.nextOneShotAtMs = this.rollOneShotDelay();
      }
    }

    const isDefault = this.currentActionId === this.defaultAction;
    if (isDefault && this.idleSchedule && this.idleSchedule.entries.length > 0) {
      this.idleAccumulatedMs += deltaMs;
      if (this.pendingAlignedOneShot) {
        // Waiting for the default loop to reach its boundary. Trigger once the
        // current tick crosses (or lands exactly on) a boundary: one-shot
        // frames reuse the phase-0 body, so the hand-off is seamless there.
        const loopMs = this.defaultLoopDurationMs();
        if (loopMs > 0) {
          const phase = this.actionElapsedMs % loopMs;
          const prevPhase = (this.actionElapsedMs - deltaMs) % loopMs;
          if (phase === 0 || phase < prevPhase) {
            this.pendingAlignedOneShot = false;
            const selected = this.pickOneShotAction();
            if (selected) {
              this.startOneShot(selected);
            } else {
              this.nextOneShotAtMs = this.rollOneShotDelay();
            }
          }
        } else {
          this.pendingAlignedOneShot = false;
        }
      } else if (this.idleAccumulatedMs >= this.nextOneShotAtMs) {
        const selected = this.pickOneShotAction();
        if (selected && this.idleSchedule.alignToDefaultLoop && this.defaultLoopDurationMs() > 0) {
          // Arm the trigger; it fires at the next default-loop boundary.
          this.pendingAlignedOneShot = true;
          this.renderDisplay();
          return;
        }
        if (selected) {
          this.startOneShot(selected);
        } else {
          this.nextOneShotAtMs = this.rollOneShotDelay();
        }
      }
    }

    this.renderDisplay();
  }

  getHitSurface(): HTMLCanvasElement {
    return this.hitCanvas;
  }

  /**
   * True once per silhouette change. 窗口裁剪区（Windows SetWindowRgn）是按 hit
   * surface 的 alpha 生成的，只在动作切换时才会变；调用方消费后应立即重新下发区域。
   */
  consumeSilhouetteDirty(): boolean {
    if (!this.silhouetteDirty) return false;
    this.silhouetteDirty = false;
    return true;
  }

  destroy(): void {
    if (this.destroyed) return;
    this.loadToken += 1;
    const clearWidth = this.viewport?.width ?? this.displayCanvas.width;
    const clearHeight = this.viewport?.height ?? this.displayCanvas.height;
    this.displayContext.clearRect(0, 0, clearWidth, clearHeight);
    this.hitContext.clearRect(0, 0, clearWidth, clearHeight);
    this.displayCanvas.style.visibility = "hidden";
    this.hitCanvas.style.visibility = "hidden";
    this.displayCanvas.remove();
    this.hitCanvas.remove();
    this.baseImage = undefined;
    this.actions = [];
    this.unionMasks = new Map();
    this.renderedHitActionId = undefined;
    this.silhouetteDirty = false;
    this.defaultAction = undefined;
    this.currentActionId = undefined;
    this.actionDefs = new Map();
    this.holdHeld = false;
    this.bounds = undefined;
    this.visible = false;
    this.destroyed = true;
  }

  private startOneShot(actionId: string): void {
    this.currentActionId = actionId;
    this.actionElapsedMs = 0;
    this.actionLoop = false;
    this.idleAccumulatedMs = 0;
    this.nextOneShotAtMs = Infinity;
  }

  private defaultLoopDurationMs(): number {
    if (!this.defaultAction) return 0;
    const loaded = this.findAction(this.defaultAction);
    if (!loaded || loaded.frames.length === 0) return 0;
    return loaded.action.frameDurationMs * loaded.frames.length;
  }

  private rollOneShotDelay(): number {
    if (!this.idleSchedule || this.idleSchedule.entries.length === 0) return Infinity;
    const schedule = this.idleSchedule;
    const min = schedule.minIntervalMs ?? Math.min(...schedule.entries.map((e) => e.minIntervalMs));
    const max = schedule.maxIntervalMs ?? Math.max(...schedule.entries.map((e) => e.maxIntervalMs));
    if (max <= min) return this.idleAccumulatedMs + min;
    return this.idleAccumulatedMs + min + this.random() * (max - min);
  }

  private pickOneShotAction(): string | null {
    if (!this.idleSchedule) return null;
    const candidates = this.idleSchedule.entries.filter((entry) => {
      if (!this.actionExists(entry.actionId)) return false;
      if (entry.actionId === this.defaultAction) return false;
      if (entry.weight <= 0) return false;
      if (
        entry.actionId === this.lastOneShotActionId
        && this.idleAccumulatedMs - this.lastOneShotAtMs < entry.minIntervalMs
      ) {
        return false;
      }
      return true;
    });
    if (candidates.length === 0) return null;
    const totalWeight = candidates.reduce((sum, entry) => sum + entry.weight, 0);
    if (totalWeight <= 0) return null;
    let roll = this.random() * totalWeight;
    for (const entry of candidates) {
      roll -= entry.weight;
      if (roll <= 0) return entry.actionId;
    }
    return candidates[candidates.length - 1]!.actionId;
  }

  private actionExists(actionId: string): boolean {
    return this.actions.some((loaded) => loaded.action.actionId === actionId);
  }

  private findAction(actionId: string): LoadedAction | undefined {
    return this.actions.find((loaded) => loaded.action.actionId === actionId);
  }

  private recomputeLayout(): void {
    if (!this.baseImage || !this.viewport) {
      this.bounds = undefined;
      return;
    }
    this.bounds = computeContainRect(this.baseImage, this.viewport);
  }

  private renderDisplay(): void {
    if (!this.viewport || !this.bounds || this.destroyed) return;
    const loaded = this.currentActionId ? this.findAction(this.currentActionId) : undefined;
    const frame = this.currentFrameImage(loaded);
    this.displayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    if (frame) {
      this.displayContext.drawImage(
        frame,
        this.bounds.x,
        this.bounds.y,
        this.bounds.width,
        this.bounds.height,
      );
    } else if (this.baseImage) {
      this.displayContext.drawImage(
        this.baseImage,
        this.bounds.x,
        this.bounds.y,
        this.bounds.width,
        this.bounds.height,
      );
    }
    this.syncHitSurface();
  }

  /**
   * hit surface 画的是"当前动作所有帧的并集轮廓"，不是 baseImage（f0000）。
   * 原因：SetWindowRgn 会按 hit surface 裁剪窗口绘制区，若轮廓固定在首帧，
   * 打哈欠下沉 / 摇尾摆出首帧之外的像素就会被窗口裁掉（内容显示不全）。
   * 只在动作切换时重画，一帧一次 drawImage，无逐帧开销。
   */
  private syncHitSurface(): void {
    if (this.renderedHitActionId === this.currentActionId) return;
    this.renderHitSurface();
    // 布局尚未就绪（load 早于 resize）时 renderHitSurface 是空操作，留到下次再试。
    if (!this.viewport || !this.bounds) return;
    this.renderedHitActionId = this.currentActionId;
    this.silhouetteDirty = true;
  }

  private currentUnionMask(): FrameImage | undefined {
    if (!this.currentActionId || !this.createMaskCanvas) return undefined;
    const cached = this.unionMasks.get(this.currentActionId);
    if (cached) return cached;
    const loaded = this.findAction(this.currentActionId);
    if (!loaded || loaded.frames.length === 0) return undefined;
    const mask = this.buildUnionMask(loaded);
    if (mask) this.unionMasks.set(this.currentActionId, mask);
    return mask;
  }

  /** 把动作内所有帧叠加成一张并集轮廓（alpha 取"任意帧不透明"）。 */
  private buildUnionMask(loaded: LoadedAction): FrameImage | undefined {
    const first = loaded.frames[0]!;
    const width = Math.max(1, Math.round(first.width));
    const height = Math.max(1, Math.round(first.height));
    let canvas: HTMLCanvasElement;
    try {
      canvas = this.createMaskCanvas!();
    } catch {
      return undefined;
    }
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return undefined;
    context.imageSmoothingEnabled = false;
    // "lighter" 的 alpha 合成为 min(1, a_src + a_dst)：某像素只要在任何一帧里
    // 不透明，并集后就不透明 —— 正是动作期间窗口不能裁掉的范围。
    context.globalCompositeOperation = "lighter";
    for (const frame of loaded.frames) context.drawImage(frame, 0, 0, width, height);
    return canvas;
  }

  private currentFrameImage(loaded: LoadedAction | undefined): FrameImage | undefined {
    if (!loaded || loaded.frames.length === 0) return undefined;
    const duration = loaded.action.frameDurationMs;
    const holdRange = this.currentActionId
      ? this.actionDefs.get(this.currentActionId)?.holdRange
      : undefined;
    let index: number;
    if (this.holdHeld && holdRange) {
      const [lo, hi] = holdRange;
      const holdStartMs = lo * duration;
      if (this.actionElapsedMs >= holdStartMs) {
        // 已进入 hold 窗口：只在 [lo, hi] 内循环（悬空保持，等待松手）。
        const windowLength = hi - lo + 1;
        const offset = Math.floor((this.actionElapsedMs - holdStartMs) / duration);
        index = lo + (offset % windowLength);
      } else {
        // 拎起段：线性前进到窗口起点。
        index = Math.min(lo, Math.floor(this.actionElapsedMs / duration));
      }
    } else {
      index = Math.floor(this.actionElapsedMs / duration) % loaded.frames.length;
    }
    return loaded.frames[index]!;
  }

  private renderHitSurface(): void {
    if (!this.viewport || !this.bounds || this.destroyed) return;
    this.hitContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    const source = this.currentUnionMask() ?? this.baseImage;
    if (!source) return;
    this.hitContext.drawImage(
      source,
      this.bounds.x,
      this.bounds.y,
      this.bounds.width,
      this.bounds.height,
    );
  }

  private assertAlive(): void {
    if (this.destroyed) throw new Error("FrameSequenceRenderer has been destroyed");
  }
}
