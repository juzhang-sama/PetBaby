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
  /** Maximum number of decoded action groups retained, including the default action. */
  maxCachedActions?: number;
  /**
   * 一次性动作播完、切回待机时的**收尾溶解时长**（毫秒）。传 0 = 关掉（硬切）。
   *
   * 为什么需要它：切回待机时是硬切，而硬切好看的前提是「动作末帧 == 待机 phase-0
   * 那一帧的姿态」。生成侧给不了这个保证 —— 实测末帧 vs 待机锚点的 IoU 在
   * 0.55~0.99 之间随机摆（暹罗 0.99 看不见跳，毛球 0.55 就是明显的「闪一下」）。
   * 溶解把这一帧的跳变摊到 ~250ms，接缝就看不见了。
   */
  tailBlendMs?: number;
}

/** 收尾溶解的默认时长：6 帧 @42ms，够短（不像慢动作）、够长（看不出接缝）。 */
export const DEFAULT_TAIL_BLEND_MS = 250;

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
  private readonly maxCachedActions: number;
  private readonly tailBlendMs: number;

  private baseImage: FrameImage | undefined;
  private actions: LoadedAction[] = [];
  private defaultAction: string | undefined;
  /**
   * 常驻动作：默认动作 + 交互动作（grab-release 这类不在 idleSchedule 里的）。
   * 交互动作是用户主动触发的（拖拽拎起），首次触发才解码会出现肉眼可见的延迟，
   * 所以后台预热并永不淘汰；偶发动作（idleSchedule 里的 yawn/lick）触发时机本来
   * 就是随机的，按需加载的那一两百毫秒用户感知不到，仍走按需 + LRU 淘汰。
   */
  private residentActionIds = new Set<string>();
  private actionCatalog = new Map<string, FrameSequenceRenderAction>();
  private actionUseClock = 0;
  private actionLastUsed = new Map<string, number>();
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
  // 收尾溶解：一次性动作播完时，把动作最后一帧暂存下来叠在待机帧之上淡出。
  private tailBlendImage: FrameImage | undefined;
  private tailBlendElapsedMs = 0;
  /**
   * 上一 tick **真正画出去的**那一帧。不能用 `currentFrameImage()` 现算：
   * 动作播完的那一 tick 下标已经绕回 0 了，现算会拿到首帧而不是刚还显示在屏幕上的末帧。
   */
  private lastRenderedFrame: FrameImage | undefined;

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
    if (options.maxCachedActions !== undefined
      && (!Number.isInteger(options.maxCachedActions) || options.maxCachedActions < 2)) {
      throw new RangeError("maxCachedActions must be an integer >= 2");
    }
    this.maxCachedActions = options.maxCachedActions ?? 2;
    const tailBlend = options.tailBlendMs ?? DEFAULT_TAIL_BLEND_MS;
    if (!Number.isFinite(tailBlend) || tailBlend < 0) {
      throw new RangeError("tailBlendMs must be a non-negative finite number");
    }
    this.tailBlendMs = tailBlend;
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
    // 换素材时让上一批尚未完成的按需加载失效，避免旧动作的解码结果切进新素材。
    this.actionGeneration += 1;
    try {
      const baseImage = await this.loadImage(asset.baseImageUrl);
      if (this.destroyed || loadToken !== this.loadToken) return;

      // 只阻塞默认动作；其他动作在首次使用时加载，避免启动时解码全部帧。
      const defaultActionDef = asset.actions.find((a) => a.actionId === asset.defaultAction);
      if (!defaultActionDef) {
        throw new Error(`default action is not declared: ${asset.defaultAction}`);
      }
      const frames: FrameImage[] = [];
      for (const url of defaultActionDef.frameUrls) frames.push(await this.loadImage(url));
      if (this.destroyed || loadToken !== this.loadToken) return;

      this.baseImage = baseImage;
      this.actions = [{ action: defaultActionDef, frames }];
      this.defaultAction = asset.defaultAction;
      this.actionCatalog = new Map(asset.actions.map((action) => [action.actionId, action]));
      this.actionUseClock = 0;
      this.actionLastUsed = new Map([[asset.defaultAction, ++this.actionUseClock]]);
      this.actionDefs = new Map(
        asset.actions.map((a) => [a.actionId, { frameDurationMs: a.frameDurationMs, holdRange: a.holdRange }]),
      );
      // 交互动作 = 没登记进 idleSchedule 的动作。默认动作常驻，其余（偶发）按需。
      const scheduled = new Set((asset.idleSchedule?.entries ?? []).map((entry) => entry.actionId));
      this.residentActionIds = new Set(
        asset.actions
          .filter((a) => a.actionId === asset.defaultAction || !scheduled.has(a.actionId))
          .map((a) => a.actionId),
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
      this.tailBlendImage = undefined;
      this.tailBlendElapsedMs = 0;
      this.lastRenderedFrame = undefined;
      this.root.replaceChildren(this.displayCanvas, this.hitCanvas);
      this.recomputeLayout();
      this.renderDisplay();

      // 后台预热交互动作（默认动作之外），保证用户第一次拖拽就有帧可播。
      this.backgroundLoad = Promise.resolve();
      for (const actionId of this.residentActionIds) {
        if (actionId === asset.defaultAction) continue;
        this.backgroundLoad = this.backgroundLoad.then(() => this.ensureActionLoaded(actionId));
      }
    } catch (error) {
      if (this.destroyed || loadToken !== this.loadToken) return;
      throw error;
    }
  }

  /** 等待当前动作的按需加载完成。 */
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
    // playMotion（landed）→ 不重置动作，把进度快照到 hold 窗口末尾，立即续播"放下+稳定"尾段。
    const releasing = this.holdHeld && this.currentActionId === actionId
      && !(options?.loop ?? false);
    this.holdHeld = false;
    this.idleAccumulatedMs = 0;
    this.pendingAlignedOneShot = false;
    const loop = options?.loop ?? (motion === "idle");

    if (this.findAction(actionId)) {
      this.touchAction(actionId);
      this.switchAction(actionId, loop, releasing);
    } else {
      // 动作还没解码完：不要立刻把显示切过去（否则会先画一帧 base/空帧再跳变）。
      // 继续播放当前待机动作，解码完成且这次请求仍是最新时才切换。
      this.backgroundLoad = this.backgroundLoad.then(async () => {
        await this.ensureActionLoaded(actionId);
        if (this.destroyed || generation !== this.actionGeneration) return;
        if (!this.findAction(actionId)) return;
        this.switchAction(actionId, loop, releasing);
        // 显示已切到新动作，上一轮的"当前动作"不再受保护，现在才淘汰它。
        this.evictActions();
        this.renderDisplay();
      });
    }
    let active = true;
    return {
      cancel: () => {
        if (!active) return;
        active = false;
        if (generation === this.actionGeneration && this.defaultAction) {
          // 递增 generation，让尚未完成的按需加载结果失效，避免它把动作切回来。
          this.actionGeneration += 1;
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

    // 「本 tick 开始时就已经在溶解了」的快照：本 tick 刚起头的溶解不该再吃一次 delta。
    const blending = this.tailBlendImage !== undefined;
    const loaded = this.findAction(this.currentActionId);
    if (loaded) {
      const duration = loaded.action.frameDurationMs * loaded.frames.length;
      if (!this.actionLoop && this.actionElapsedMs >= duration) {
        this.lastOneShotActionId = this.currentActionId;
        this.lastOneShotAtMs = this.idleAccumulatedMs;
        // 收尾溶解：把动作**此刻显示在屏幕上的那一帧**留下来，叠在待机帧之上淡出。
        // 生成侧的落点有随机性（末帧 vs 待机锚点 IoU 0.55~0.99），硬切就是「闪一下」。
        this.beginTailBlend();
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
    if (blending) {
      this.tailBlendElapsedMs += deltaMs;
      if (this.tailBlendElapsedMs >= this.tailBlendMs) {
        this.tailBlendImage = undefined;
        this.tailBlendElapsedMs = 0;
        // 溶解结束 ⇒ 窗口轮廓可以收回待机那一支了（下面 renderDisplay 会重烘焙）。
        this.renderedHitActionId = undefined;
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
    this.actionCatalog = new Map();
    this.residentActionIds = new Set();
    this.actionLastUsed = new Map();
    this.unionMasks = new Map();
    this.renderedHitActionId = undefined;
    this.silhouetteDirty = false;
    this.tailBlendImage = undefined;
    this.tailBlendElapsedMs = 0;
    this.lastRenderedFrame = undefined;
    this.defaultAction = undefined;
    this.currentActionId = undefined;
    this.actionDefs = new Map();
    this.holdHeld = false;
    this.bounds = undefined;
    this.visible = false;
    this.destroyed = true;
  }

  /** 真正把"正在显示的动作"切换过去；releasing 时进度快照到 hold 窗口末尾。 */
  private switchAction(actionId: string, loop: boolean, releasing: boolean): void {
    // 开始播新动作 → 上一次的收尾溶解作废（否则旧动作末帧会盖在新动作上）。
    this.tailBlendImage = undefined;
    this.tailBlendElapsedMs = 0;
    this.currentActionId = actionId;
    this.actionLoop = loop;
    if (releasing) {
      const def = this.actionDefs.get(actionId);
      const holdRange = def?.holdRange;
      const duration = def?.frameDurationMs ?? 0;
      this.actionElapsedMs = holdRange && duration > 0 ? (holdRange[1] + 1) * duration : 0;
    } else {
      this.actionElapsedMs = 0;
    }
    // loop + 动作声明 holdRange → 交互保持：播放到区间后只在窗口内循环悬空。
    if (loop && this.actionDefs.get(actionId)?.holdRange) {
      this.holdHeld = true;
    }
  }

  private startOneShot(actionId: string): void {
    this.idleAccumulatedMs = 0;
    this.nextOneShotAtMs = Infinity;
    if (this.findAction(actionId)) {
      this.touchAction(actionId);
      this.switchAction(actionId, false, false);
      return;
    }
    // 首次触发的动作还没解码：继续播待机，解码完成后才从第 0 帧接上，
    // 期间不能把 actionElapsedMs 提前累计进去。
    const generation = this.actionGeneration;
    this.backgroundLoad = this.backgroundLoad.then(async () => {
      await this.ensureActionLoaded(actionId);
      if (this.destroyed || generation !== this.actionGeneration) return;
      if (!this.findAction(actionId)) return;
      this.switchAction(actionId, false, false);
      this.evictActions();
      this.renderDisplay();
    });
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
    return this.actionCatalog.has(actionId);
  }

  private findAction(actionId: string): LoadedAction | undefined {
    return this.actions.find((loaded) => loaded.action.actionId === actionId);
  }

  private touchAction(actionId: string): void {
    if (this.findAction(actionId)) this.actionLastUsed.set(actionId, ++this.actionUseClock);
  }

  private async ensureActionLoaded(actionId: string): Promise<void> {
    if (this.destroyed || !this.defaultAction) return;
    if (this.findAction(actionId)) {
      this.touchAction(actionId);
      return;
    }
    const action = this.actionCatalog.get(actionId);
    if (!action) return;
    const loadToken = this.loadToken;
    const frames: FrameImage[] = [];
    for (const url of action.frameUrls) {
      if (this.destroyed || loadToken !== this.loadToken) return;
      frames.push(await this.loadImage(url));
    }
    if (this.destroyed || loadToken !== this.loadToken) return;
    this.actions = this.actions.filter((loaded) => loaded.action.actionId !== actionId);
    this.actions.push({ action, frames });
    this.touchAction(actionId);
    this.evictActions(actionId);
  }

  private evictActions(protectedId?: string): void {
    // 刚解码完、但显示还没切过去的动作也必须保护，否则会被自己这一轮淘汰掉。
    const protectedIds = new Set([
      ...this.residentActionIds,
      this.defaultAction,
      this.currentActionId,
      protectedId,
    ]);
    // 常驻动作不受缓存上限约束（它们是交互零延迟的硬需求）；上限只约束偶发动作。
    const limit = Math.max(this.maxCachedActions, this.residentActionIds.size);
    while (this.actions.length > limit) {
      const candidate = this.actions
        .filter((loaded) => !protectedIds.has(loaded.action.actionId))
        .sort((left, right) =>
          (this.actionLastUsed.get(left.action.actionId) ?? 0)
          - (this.actionLastUsed.get(right.action.actionId) ?? 0)
        )[0];
      if (!candidate) return;
      const actionId = candidate.action.actionId;
      this.actions = this.actions.filter((loaded) => loaded.action.actionId !== actionId);
      this.actionLastUsed.delete(actionId);
      this.unionMasks.delete(actionId);
    }
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
    if (frame) this.lastRenderedFrame = frame;
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
    // 收尾溶解：待机帧画完之后，把动作末帧按「剩余不透明度」叠在上面。
    // source-over 的合成结果是 old*a + new*(1-a)，总 alpha 恒为 1 ⇒ 半透明窗口
    // 不会在溶解中途「透出桌面」（反过来画就会，试过，别改）。
    if (frame && this.tailBlendImage) {
      const remaining = 1 - this.tailBlendElapsedMs / this.tailBlendMs;
      if (remaining > 0) {
        this.displayContext.globalAlpha = remaining;
        this.displayContext.drawImage(
          this.tailBlendImage,
          this.bounds.x,
          this.bounds.y,
          this.bounds.width,
          this.bounds.height,
        );
        this.displayContext.globalAlpha = 1;
      }
    }
    this.syncHitSurface();
  }

  /** 起一段收尾溶解。`tailBlendMs = 0`（显式关掉）时什么都不做。 */
  private beginTailBlend(): void {
    if (this.tailBlendMs <= 0 || !this.lastRenderedFrame) return;
    this.tailBlendImage = this.lastRenderedFrame;
    this.tailBlendElapsedMs = 0;
  }

  /**
   * hit surface 画的是"当前动作所有帧的并集轮廓"，不是 baseImage（f0000）。
   * 原因：SetWindowRgn 会按 hit surface 裁剪窗口绘制区，若轮廓固定在首帧，
   * 打哈欠下沉 / 摇尾摆出首帧之外的像素就会被窗口裁掉（内容显示不全）。
   * 只在动作切换时重画，一帧一次 drawImage，无逐帧开销。
   */
  private syncHitSurface(): void {
    if (this.renderedHitActionId === this.currentActionId) return;
    // 收尾溶解期间**沿用动作那一支的轮廓**：窗口裁剪区（SetWindowRgn）按轮廓切，
    // 此刻画面里还叠着动作末帧，换成待机轮廓会把它超出待机的那部分裁掉。
    if (this.tailBlendImage && this.renderedHitActionId !== undefined) return;
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
