import { describe, expect, it, vi } from "vitest";
import { DEFAULT_TAIL_BLEND_MS, FrameSequenceRenderer } from "./frame-sequence-renderer";
import type { PetCalibrationV1 } from "./pet-calibration";
import type { PetRenderAsset } from "./pet-renderer";

type FrameSequenceAsset = Extract<PetRenderAsset, { kind: "frame-sequence" }>;

function frameAsset(overrides: Partial<FrameSequenceAsset> = {}): FrameSequenceAsset {
  return {
    kind: "frame-sequence",
    baseImageUrl: "body.png",
    actions: [
      {
        actionId: "breath",
        loop: true,
        frameDurationMs: 180,
        frameUrls: ["breath/f00.png", "breath/f01.png", "breath/f02.png"],
      },
      {
        actionId: "blink",
        loop: true,
        frameDurationMs: 150,
        frameUrls: ["blink/f00.png", "blink/f01.png"],
      },
      {
        actionId: "tail-wag",
        loop: true,
        frameDurationMs: 120,
        frameUrls: ["tail/f00.png", "tail/f01.png"],
      },
    ],
    defaultAction: "breath",
    semantics: { idle: "breath", blink: "blink", "react-happy": "tail-wag" },
    idleSchedule: {
      entries: [{ actionId: "blink", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 }],
    },
    hitBounds: { left: 0.1, top: 0.1, right: 0.9, bottom: 0.9 },
    ...overrides,
  };
}

function rendererHarness(
  overrides: Partial<FrameSequenceAsset> = {},
  options: { maxCachedActions?: number; tailBlendMs?: number } = {},
) {
  const contexts = Array.from({ length: 2 }, () => ({
    clearRect: vi.fn(),
    drawImage: vi.fn(),
    setTransform: vi.fn(),
    globalAlpha: 1,
  }));
  // 记录 display context 的 globalAlpha 写入序列 —— 收尾溶解的淡出靠它，
  // 而它画完就被复位成 1，只能在 setter 里抓。
  const alphaLog: number[] = [];
  let alpha = 1;
  Object.defineProperty(contexts[0], "globalAlpha", {
    get: () => alpha,
    set: (value: number) => {
      alpha = value;
      alphaLog.push(value);
    },
  });
  const canvases = contexts.map((context) => ({
    width: 0,
    height: 0,
    style: {} as CSSStyleDeclaration,
    getContext: vi.fn(() => context),
    remove: vi.fn(),
  } as unknown as HTMLCanvasElement));
  // 并集轮廓用的离屏画布：每次烘焙一张新的，模拟 document.createElement。
  const maskContexts: Array<{
    clearRect: ReturnType<typeof vi.fn>;
    drawImage: ReturnType<typeof vi.fn>;
    imageSmoothingEnabled: boolean;
    globalCompositeOperation: string;
  }> = [];
  const maskCanvases: HTMLCanvasElement[] = [];
  const createMaskCanvas = (): HTMLCanvasElement => {
    const context = {
      clearRect: vi.fn(),
      drawImage: vi.fn(),
      setTransform: vi.fn(),
      imageSmoothingEnabled: true,
      globalCompositeOperation: "source-over",
    };
    const canvas = {
      width: 0,
      height: 0,
      style: {} as CSSStyleDeclaration,
      getContext: vi.fn(() => context),
      remove: vi.fn(),
    } as unknown as HTMLCanvasElement;
    maskContexts.push(context);
    maskCanvases.push(canvas);
    return canvas;
  };
  let canvasIndex = 0;
  const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
  const imageByUrl = new Map<string, CanvasImageSource & { width: number; height: number }>();
  const loadImage = vi.fn(async (url: string) => {
    let image = imageByUrl.get(url);
    if (!image) {
      image = { width: 1000, height: 1000 } as CanvasImageSource & { width: number; height: number };
      imageByUrl.set(url, image);
    }
    return image;
  });
  const random = vi.fn(() => 0.5);
  const renderer = new FrameSequenceRenderer(root, {
    createCanvas: () => canvases[canvasIndex++]!,
    createMaskCanvas,
    loadImage,
    random,
    // 缓存淘汰是独立关注点，由专项测试用 maxCachedActions: 2 覆盖；
    // 行为测试给足缓存，避免淘汰策略把断言要用的动作提前清掉。
    maxCachedActions: options.maxCachedActions ?? 10,
    tailBlendMs: options.tailBlendMs ?? DEFAULT_TAIL_BLEND_MS,
  });
  renderer.setVisibility(true);
  const asset = frameAsset(overrides);
  return {
    renderer,
    root,
    loadImage,
    random,
    asset,
    // 默认只加载 base + 默认动作（按需加载语义）。
    load: async () => {
      await renderer.load(asset);
    },
    // 需要非默认动作已解码时，显式触发每个语义动作的按需加载。
    loadAll: async () => {
      await renderer.load(asset);
      for (const action of asset.actions) {
        if (action.actionId === asset.defaultAction) continue;
        const motion = Object.entries(asset.semantics).find(([, actionId]) => actionId === action.actionId)?.[0];
        if (!motion) continue;
        renderer.playMotion(motion as "idle");
        await renderer.whenReady();
      }
      renderer.playMotion("idle", { loop: true });
    },
    context: contexts[0]!,
    alphaLog,
    hitContext: contexts[1]!,
    displayCanvas: canvases[0]!,
    hitCanvas: canvases[1]!,
    imageByUrl,
    maskContexts,
    maskCanvases,
  };
}

describe("FrameSequenceRenderer", () => {
  it("loads the base image and every action frame", async () => {
    const test = rendererHarness();
    await test.loadAll();
    expect(test.loadImage).toHaveBeenCalledTimes(1 + 3 + 2 + 2);
    expect(test.root.replaceChildren).toHaveBeenCalledWith(test.displayCanvas, test.hitCanvas);
  });

  it("loads only the default action until another action is requested", async () => {
    const test = rendererHarness();
    // 只 await load()：默认动作 breath（3 帧）+ baseImage 已加载，可立即渲染首帧。
    await test.renderer.load(test.asset);
    expect(test.loadImage).toHaveBeenCalledWith(test.asset.baseImageUrl);
    for (const url of ["breath/f00.png", "breath/f01.png", "breath/f02.png"]) {
      expect(test.loadImage).toHaveBeenCalledWith(url);
    }
    expect(test.root.replaceChildren).toHaveBeenCalledWith(test.displayCanvas, test.hitCanvas);

    // tail-wag 是交互动作（不在 idleSchedule 里），后台预热后可立即播放，不占用户时间。
    await test.renderer.whenReady();
    expect(test.loadImage).toHaveBeenCalledWith("tail/f00.png");
    // 偶发动作在没人触发前绝不解码。
    expect(test.loadImage).not.toHaveBeenCalledWith("blink/f00.png");

    test.renderer.playMotion("blink" as "idle");
    await test.renderer.whenReady();
    expect(test.loadImage).toHaveBeenCalledWith("blink/f00.png");
    expect(test.loadImage).toHaveBeenCalledTimes(1 + 3 + 2 + 2);
  });

  it("preloads interactive actions so the first drag has no decode delay", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.renderer.load(test.asset);
    await test.renderer.whenReady();

    // 交互动作已预热 → 请求时同步切换，不用等解码。
    test.renderer.playMotion("react-happy");
    test.renderer.update(1);
    expect(test.context.drawImage.mock.calls.at(-1)![0])
      .toBe(test.imageByUrl.get("tail/f00.png"));
  });

  it("keeps the default and current actions resident and reloads an evicted action on demand", async () => {
    // breath 是默认动作；blink / yawn 都登记在 idleSchedule 里（偶发），
    // 所以常驻只有 breath，maxCachedActions: 2 会在这两个偶发动作之间淘汰。
    const test = rendererHarness(
      {
        actions: [
          {
            actionId: "breath",
            loop: true,
            frameDurationMs: 180,
            frameUrls: ["breath/f00.png", "breath/f01.png", "breath/f02.png"],
          },
          { actionId: "blink", loop: true, frameDurationMs: 150, frameUrls: ["blink/f00.png", "blink/f01.png"] },
          { actionId: "yawn", loop: false, frameDurationMs: 150, frameUrls: ["yawn/f00.png", "yawn/f01.png"] },
        ],
        semantics: { idle: "breath", blink: "blink", "react-curious": "yawn" },
        idleSchedule: {
          entries: [
            { actionId: "blink", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 },
            { actionId: "yawn", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 },
          ],
        },
      },
      { maxCachedActions: 2 },
    );
    await test.renderer.load(test.asset);

    test.renderer.playMotion("blink" as "idle");
    await test.renderer.whenReady();
    test.renderer.playMotion("react-curious"); // semantics -> yawn
    await test.renderer.whenReady();
    test.renderer.playMotion("blink" as "idle");
    await test.renderer.whenReady();

    // 默认动作 + 当前动作是缓存上限；blink 被 yawn 淘汰后再次请求会重新解码。
    expect(test.loadImage).toHaveBeenCalledTimes(1 + 3 + 2 + 2 + 2);
  });

  it("never evicts interactive actions, so dragging stays instant", async () => {
    // tail-wag 是交互动作；缓存上限 2 时，反复触发偶发动作也不能把它挤掉。
    const test = rendererHarness({}, { maxCachedActions: 2 });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.renderer.load(test.asset);
    await test.renderer.whenReady();
    const before = test.loadImage.mock.calls.length;

    for (let i = 0; i < 3; i += 1) {
      test.renderer.playMotion("blink" as "idle");
      await test.renderer.whenReady();
    }
    test.renderer.playMotion("react-happy");
    test.renderer.update(1);
    // 没有重新解码 tail-wag 帧（loadImage 调用数不变），且立刻切到了 tail 首帧。
    expect(test.loadImage.mock.calls.length).toBe(before + 2); // 只多了 blink 一次解码
    expect(test.context.drawImage.mock.calls.at(-1)![0])
      .toBe(test.imageByUrl.get("tail/f00.png"));
  });

  it("keeps playing the current action until the requested action finishes decoding", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.renderer.load(test.asset);
    const breathFrame = test.context.drawImage.mock.calls.at(-1)![0];

    // 请求一个还没解码的动作：显示不能立刻切过去（会先画空帧再跳变）。
    test.renderer.playMotion("react-happy");
    test.renderer.update(1);
    expect(test.context.drawImage.mock.calls.at(-1)![0]).toBe(breathFrame);

    // 解码完成后且请求仍然有效，才从第 0 帧接上。
    await test.renderer.whenReady();
    expect(test.context.drawImage.mock.calls.at(-1)![0])
      .toBe(test.imageByUrl.get("tail/f00.png"));
  });

  it("drops a pending load result when the motion is cancelled mid-decode", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.renderer.load(test.asset);

    const handle = test.renderer.playMotion("react-happy");
    handle.cancel();
    await test.renderer.whenReady();
    test.renderer.update(1);
    // 取消后回到默认动作，迟到的解码结果不得把显示切到 tail-wag。
    expect(test.context.drawImage.mock.calls.at(-1)![0])
      .toBe(test.imageByUrl.get("breath/f00.png"));
  });

  it("starts breathing from the first frame after load", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.load();
    expect(test.context.drawImage).toHaveBeenCalledTimes(1);
  });

  it("advances through breath frames in order while idle", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.load();
    test.context.drawImage.mockClear();
    test.renderer.update(180);
    const frameAt180 = test.context.drawImage.mock.calls[0]![0];
    test.context.drawImage.mockClear();
    test.renderer.update(180);
    const frameAt360 = test.context.drawImage.mock.calls[0]![0];
    expect(frameAt180).not.toBe(frameAt360);
  });

  it("loops breath frames back to the first frame after a full cycle", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.load();
    const firstFrame = test.context.drawImage.mock.calls[0]![0];
    test.context.drawImage.mockClear();
    test.renderer.update(540);
    const wrapped = test.context.drawImage.mock.calls[0]![0];
    expect(wrapped).toBe(firstFrame);
  });

  it("maps a react motion to tail-wag and returns to the default action after one cycle", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();
    const handle = test.renderer.playMotion("react-happy");
    test.context.drawImage.mockClear();
    test.renderer.update(120);
    const tailFrame0 = test.context.drawImage.mock.calls[0]![0];
    expect(tailFrame0).toBe(test.imageByUrl.get("tail/f01.png"));
    // 再走 120 ⇒ 越过 tail-wag 总时长（2×120），回归待机；动作末帧叠在上面做收尾溶解，
    // 所以「当前动作帧」看本 tick 的第一次 drawImage，不是 at(-1)。
    test.context.drawImage.mockClear();
    test.renderer.update(120);
    const backToBreath = test.context.drawImage.mock.calls[0]![0];
    expect(backToBreath).toBe(test.imageByUrl.get("breath/f00.png"));
    test.renderer.update(120);
    test.context.drawImage.mockClear();
    test.renderer.update(180);
    const afterCycle = test.context.drawImage.mock.calls[0]![0];
    const breathFrames = ["breath/f00.png", "breath/f01.png", "breath/f02.png"].map(
      (url) => test.imageByUrl.get(url),
    );
    expect(breathFrames).toContain(afterCycle); // 一个周期后回到待机，不再播 tail-wag
    handle.cancel();
  });

  it("falls back to the default action for unmapped motions", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.load();
    test.renderer.playMotion("look-left");
    test.context.drawImage.mockClear();
    test.renderer.update(180);
    expect(test.context.drawImage).toHaveBeenCalledTimes(1);
  });

  it("triggers a blink after the configured interval and resumes breathing", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();
    test.context.drawImage.mockClear();
    test.renderer.update(2500);
    test.renderer.update(150);
    const blinkFrame0 = test.context.drawImage.mock.calls.at(-1)![0];
    expect(blinkFrame0).toBeDefined();
    test.context.drawImage.mockClear();
    test.renderer.update(150);
    const blinkFrame1 = test.context.drawImage.mock.calls[0]![0];
    expect(blinkFrame1).not.toBe(blinkFrame0);
  });

  it("does not blink when disabled", async () => {
    const test = rendererHarness({ idleSchedule: null });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.load();
    test.context.drawImage.mockClear();
    test.renderer.update(100000);
    expect(test.context.drawImage).toHaveBeenCalledTimes(1);
  });

  it("reports hit areas only inside hitBounds", async () => {
    const test = rendererHarness();
    test.renderer.resize({ width: 400, height: 500, dpr: 1 });
    await test.load();
    expect(test.renderer.hitTest({ x: 200, y: 250 })).toBe("body");
    expect(test.renderer.hitTest({ x: 0, y: 0 })).toBeNull();
    expect(test.renderer.hitTest({ x: 359, y: 409 })).toBe("body");
    expect(test.renderer.hitTest({ x: 360, y: 100 })).toBeNull();
  });

  it("ignores calibration and expression calls without throwing", async () => {
    const test = rendererHarness();
    await test.load();
    expect(() => test.renderer.setExpression("happy")).not.toThrow();
    expect(() => test.renderer.setCalibration({} as PetCalibrationV1)).not.toThrow();
    expect(() => test.renderer.setLookTarget({ x: 0.5, y: 0.5 })).not.toThrow();
    expect(() => test.renderer.setLipSync(0.5)).not.toThrow();
  });

  it("destroy removes canvases and stops drawing", async () => {
    const test = rendererHarness();
    await test.load();
    test.renderer.destroy();
    expect(test.displayCanvas.remove).toHaveBeenCalled();
    expect(test.hitCanvas.remove).toHaveBeenCalled();
  });

  it("picks a one-shot action from idleSchedule by weight", async () => {
    const test = rendererHarness({
      idleSchedule: {
        entries: [
          { actionId: "blink", weight: 0, minIntervalMs: 2500, maxIntervalMs: 2500 },
          { actionId: "tail-wag", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 },
        ],
      },
    });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();
    test.context.drawImage.mockClear();
    test.renderer.update(2500); // 到点触发 tail-wag（weight 1）
    test.context.drawImage.mockClear();
    test.renderer.update(120);
    const tailFrame0 = test.context.drawImage.mock.calls[0]![0];
    expect(tailFrame0).toBeDefined();
    // 越过 tail-wag 总时长 ⇒ 回归待机（动作末帧叠在上面做收尾溶解 ⇒ 看 calls[0]）。
    test.context.drawImage.mockClear();
    test.renderer.update(120);
    const tailFrame1 = test.context.drawImage.mock.calls[0]![0];
    expect(tailFrame1).not.toBe(tailFrame0);
  });

  it("respects per-action minIntervalMs in idleSchedule", async () => {
    const test = rendererHarness({
      idleSchedule: {
        entries: [
          { actionId: "tail-wag", weight: 1, minIntervalMs: 5000, maxIntervalMs: 5000 },
        ],
      },
    });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();
    test.context.drawImage.mockClear();
    test.renderer.update(2500);
    // tail-wag minInterval is 5000, so no action should trigger yet:
    // 呼吸仍在正常循环（画的是 breath 帧），而不是切到 tail-wag。
    test.renderer.update(120);
    const frame = test.context.drawImage.mock.calls.at(-1)![0];
    expect(frame).not.toBe(test.imageByUrl.get("tail/f00.png"));
    expect(frame).not.toBe(test.imageByUrl.get("tail/f01.png"));
  });

  it("defers aligned one-shots to the default loop boundary", async () => {
    // breath loop = 3 frames * 180ms = 540ms. Trigger armed at 2500ms but the
    // loop phase is 2500 % 540 = 340ms, so blink must wait for the 2700ms
    // boundary instead of firing mid-loop.
    const test = rendererHarness({
      idleSchedule: {
        entries: [{ actionId: "blink", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 }],
        alignToDefaultLoop: true,
      },
    });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();
    test.context.drawImage.mockClear();
    // Reach 2500ms: interval elapsed, phase = 340ms, trigger armed but deferred.
    test.renderer.update(2500);
    const breathFrameBeforeBoundary = test.context.drawImage.mock.calls.at(-1)![0];
    expect(breathFrameBeforeBoundary).toBeDefined();
    // Advance 100ms -> 2600ms, still before the 2700ms boundary: still breath
    // （呼吸继续循环、帧正常推进，只是还没切到眨眼）。
    test.renderer.update(100);
    const stillBreath = test.context.drawImage.mock.calls.at(-1)![0];
    expect(stillBreath).not.toBe(test.imageByUrl.get("blink/f00.png"));
    // Advance 100ms -> 2700ms exactly: boundary reached, blink fires now.
    test.renderer.update(100);
    const blinkFrame = test.context.drawImage.mock.calls.at(-1)![0];
    expect(blinkFrame).not.toBe(breathFrameBeforeBoundary);
    expect(blinkFrame).toBe(test.imageByUrl.get("blink/f00.png"));
  });

  it("fires aligned one-shot when a boundary crossing is detected within one tick", async () => {
    // Tick sizes do not divide the 540ms loop evenly: after update(2500) the
    // phase is 340ms; update(200) crosses the 2700ms boundary inside the tick,
    // so blink must fire even though no tick lands exactly on the boundary.
    const test = rendererHarness({
      idleSchedule: {
        entries: [{ actionId: "blink", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 }],
        alignToDefaultLoop: true,
      },
    });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();
    test.context.drawImage.mockClear();
    test.renderer.update(2500); // phase 340, armed
    test.renderer.update(200); // 2700: boundary crossed within this tick
    expect(test.context.drawImage.mock.calls.at(-1)![0]).toBe(test.imageByUrl.get("blink/f00.png"));
  });

  it("resumes the default loop at phase 0 after an aligned one-shot ends", async () => {
    const test = rendererHarness({
      idleSchedule: {
        entries: [{ actionId: "blink", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 }],
        alignToDefaultLoop: true,
      },
    });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();
    test.context.drawImage.mockClear();
    test.renderer.update(2500); // armed at phase 340
    test.renderer.update(100); // 2600
    test.renderer.update(100); // 2700 boundary -> blink frame 0
    test.renderer.update(150); // blink frame 1
    test.context.drawImage.mockClear();
    test.renderer.update(150); // blink done -> resume breath at phase 0
    // 收尾溶解：这一 tick 画两次 —— 先待机帧，再把动作末帧按剩余不透明度叠上去。
    // 「当前动作帧」= 第一次 drawImage，不是 at(-1)。
    const calls = test.context.drawImage.mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0]![0]).toBe(test.imageByUrl.get("breath/f00.png"));
    expect(calls[1]![0]).toBe(test.imageByUrl.get("blink/f01.png"));
  });

  // 窗口裁剪区（SetWindowRgn）按 hit surface 的 alpha 生成。若 hit surface 固定在
  // baseImage（f0000），动作期间身体形变超出首帧轮廓的像素就会被窗口裁掉。
  describe("窗口裁剪轮廓（hit surface）", () => {
    it("用当前动作所有帧的并集轮廓，而不是静态首帧", async () => {
      const test = rendererHarness();
      test.renderer.resize({ width: 400, height: 500, dpr: 2 });
      // 只加载默认动作：烘焙次数必须恰好是 1，多一次就说明又预热了别的动作。
      await test.renderer.load(test.asset);

      // 烘焙一次：breath 的 3 帧叠加（alpha 取"任意帧不透明"，所以用 lighter）。
      expect(test.maskContexts).toHaveLength(1);
      const mask = test.maskContexts[0]!;
      expect(mask.globalCompositeOperation).toBe("lighter");
      expect(mask.drawImage.mock.calls.map((call) => call[0])).toEqual([
        test.imageByUrl.get("breath/f00.png"),
        test.imageByUrl.get("breath/f01.png"),
        test.imageByUrl.get("breath/f02.png"),
      ]);
      // hit surface 画的是并集，不是 baseImage。
      const base = test.imageByUrl.get("body.png");
      expect(test.hitContext.drawImage.mock.calls.at(-1)![0]).toBe(test.maskCanvases[0]);
      expect(test.hitContext.drawImage.mock.calls.some((call) => call[0] === base)).toBe(false);
    });

    it("动作切换时重新烘焙并集，并把窗口区域标记为待刷新", async () => {
      const test = rendererHarness();
      test.renderer.resize({ width: 400, height: 500, dpr: 2 });
      await test.renderer.load(test.asset);
      expect(test.renderer.consumeSilhouetteDirty()).toBe(true);

      test.renderer.playMotion("react-happy"); // semantics -> tail-wag
      await test.renderer.whenReady(); // tail-wag 按需解码完成后才切换显示
      test.renderer.update(1);
      expect(test.renderer.consumeSilhouetteDirty()).toBe(true);
      expect(test.maskContexts).toHaveLength(2);
      expect(test.maskContexts[1]!.drawImage.mock.calls.map((call) => call[0])).toEqual([
        test.imageByUrl.get("tail/f00.png"),
        test.imageByUrl.get("tail/f01.png"),
      ]);
      expect(test.hitContext.drawImage.mock.calls.at(-1)![0]).toBe(test.maskCanvases[1]);
    });

    it("同一动作内不重复刷新窗口区域（只标记一次）", async () => {
      const test = rendererHarness();
      test.renderer.resize({ width: 400, height: 500, dpr: 2 });
      await test.renderer.load(test.asset);
      expect(test.renderer.consumeSilhouetteDirty()).toBe(true);

      test.renderer.update(180);
      test.renderer.update(180);
      test.renderer.update(180);
      expect(test.renderer.consumeSilhouetteDirty()).toBe(false);
      // 并集按动作缓存，重复帧不会重新烘焙。
      expect(test.maskContexts).toHaveLength(1);
    });

    it("视口变化后重画 hit surface 并再次标记待刷新", async () => {
      const test = rendererHarness();
      test.renderer.resize({ width: 400, height: 500, dpr: 2 });
      await test.renderer.load(test.asset);
      test.renderer.consumeSilhouetteDirty();

      test.renderer.resize({ width: 300, height: 600, dpr: 2 });
      expect(test.renderer.consumeSilhouetteDirty()).toBe(true);
      expect(test.maskContexts).toHaveLength(1); // 并集与视口无关，不重烘焙
    });
  });
});

describe("FrameSequenceRenderer 交互保持（holdRange）", () => {
  const GRAB_URLS = Array.from(
    { length: 121 },
    (_, i) => `grab/f${String(i).padStart(4, "0")}.webp`,
  );

  function grabAsset(holdRange: readonly [number, number]) {
    return frameAsset({
      actions: [
        {
          actionId: "breath",
          loop: true,
          frameDurationMs: 42,
          frameUrls: ["breath/f0000.webp"],
        },
        {
          actionId: "grab-release",
          loop: false,
          frameDurationMs: 42,
          frameUrls: GRAB_URLS,
          holdRange,
        },
      ],
      defaultAction: "breath",
      semantics: { idle: "breath", carried: "grab-release", landed: "grab-release" },
      idleSchedule: null,
    });
  }

  const frameAt = (test: ReturnType<typeof rendererHarness>, url: string) =>
    test.imageByUrl.get(url);

  it("carried 按住：拎起到窗口起点后只在 [lo,hi] 内循环悬空，永不出窗口", async () => {
    const test = rendererHarness(grabAsset([35, 63]));
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();

    test.renderer.playMotion("carried", { loop: true });
    // 拎起段线性前进：第 k 次 update(42) 后 elapsed = k*42 → 帧 k。
    for (let k = 1; k <= 63; k += 1) test.renderer.update(42);
    const holdEnd = test.context.drawImage.mock.calls.at(-1)![0];
    expect(holdEnd).toBe(frameAt(test, "grab/f0063.webp"));

    // 窗口末尾再走一帧 → 折回窗口起点（hi -> lo），绝不进入放下段 f0064。
    test.renderer.update(42);
    const wrapped = test.context.drawImage.mock.calls.at(-1)![0];
    expect(wrapped).toBe(frameAt(test, "grab/f0035.webp"));

    // 长时间按住：持续循环不得播到窗口外（放下段 f0064+ / 拎起段 f0000-34），
    // 且窗口内确实在走动（多帧可见），不是冻结在某一帧。
    test.context.drawImage.mockClear();
    for (let i = 0; i < 120; i += 1) test.renderer.update(42);
    const seenInside = new Set<CanvasImageSource>();
    for (const call of test.context.drawImage.mock.calls) {
      const drawn = call[0] as CanvasImageSource;
      const inside = GRAB_URLS.slice(35, 64).some((url) => frameAt(test, url) === drawn);
      expect(inside, "悬空循环期间不得播出窗口帧").toBe(true);
      seenInside.add(drawn);
    }
    expect(seenInside.size).toBeGreaterThan(5); // 窗口内有实际循环动作
  });

  it("landed 松手：从窗口末尾续播放下尾段，播完回归默认动作", async () => {
    const test = rendererHarness(grabAsset([35, 63]));
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();

    test.renderer.playMotion("carried", { loop: true });
    test.renderer.update(60000); // 按住远超总时长，elapsed 已无界增长
    test.renderer.playMotion("landed"); // 松手：同动作、无 loop

    // 进度被快照到 hold 窗口末尾 (hi+1)，不是从 f0000 重新播拎起段。
    test.renderer.update(1);
    const tailStart = test.context.drawImage.mock.calls.at(-1)![0];
    expect(tailStart).toBe(frameAt(test, "grab/f0064.webp"));

    // 推进到尾帧（elapsed 2689 + 2351 = 5040 = 120*42 → f0120）。
    test.renderer.update(2351);
    const tailEnd = test.context.drawImage.mock.calls.at(-1)![0];
    expect(tailEnd).toBe(frameAt(test, "grab/f0120.webp"));

    // 再跨过总时长 → 放下段播完，回归默认动作 breath（末帧叠在上面做收尾溶解）。
    test.context.drawImage.mockClear();
    test.renderer.update(42);
    const calls = test.context.drawImage.mock.calls;
    expect(calls[0]![0]).toBe(frameAt(test, "breath/f0000.webp"));
    expect(calls[1]![0]).toBe(frameAt(test, "grab/f0120.webp"));
  });

  it("拎起中途松手：立即跳到放下段", async () => {
    const test = rendererHarness(grabAsset([35, 63]));
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();

    test.renderer.playMotion("carried", { loop: true });
    for (let k = 1; k <= 10; k += 1) test.renderer.update(42); // 停在第 10 帧（< lo）
    test.renderer.playMotion("landed");
    test.renderer.update(1);
    const resumed = test.context.drawImage.mock.calls.at(-1)![0];
    expect(resumed).toBe(frameAt(test, "grab/f0064.webp"));
    expect(resumed).not.toBe(frameAt(test, "grab/f0010.webp"));
  });
});

describe("FrameSequenceRenderer 收尾溶解（tailBlend）", () => {
  // 由来：切回待机是硬切，而硬切好看的前提是「动作末帧 == 待机 phase-0 那一帧的姿态」。
  // 生成侧给不了这个保证 —— 实测末帧 vs 待机锚点 IoU 在 0.55~0.99 之间随机摆
  // （暹罗 0.99 看不见跳，毛球 0.55 就是老王说的「松手后闪一下」）。
  function blendAsset() {
    return frameAsset({
      actions: [
        { actionId: "breath", loop: true, frameDurationMs: 42, frameUrls: ["breath/f00.png"] },
        {
          actionId: "blink",
          loop: false,
          frameDurationMs: 100,
          frameUrls: ["blink/f00.png", "blink/f01.png"],
        },
      ],
      defaultAction: "breath",
      semantics: { idle: "breath", blink: "blink" },
      idleSchedule: null,
    });
  }

  const drawnUrls = (test: ReturnType<typeof rendererHarness>) =>
    test.context.drawImage.mock.calls.map((call) => {
      for (const [url, image] of test.imageByUrl) if (image === call[0]) return url;
      return "?";
    });

  it("一次性动作播完：待机帧先画，动作末帧叠在上面逐级淡出，到时收工", async () => {
    const test = rendererHarness(blendAsset());
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();

    test.renderer.playMotion("blink");
    await test.renderer.whenReady();
    test.renderer.update(1); // elapsed 1 → f00
    test.renderer.update(100); // elapsed 101 → f01（末帧）
    test.context.drawImage.mockClear();

    test.renderer.update(100); // elapsed 201 ≥ 200 ⇒ 播完，起溶解
    expect(drawnUrls(test)).toEqual(["breath/f00.png", "blink/f01.png"]);

    // 每 50ms 一跳：剩余不透明度 1 → .8 → .6 → .4 → .2 → 收工。
    const seen: number[] = [];
    for (let step = 0; step < 5; step += 1) {
      test.context.drawImage.mockClear();
      test.alphaLog.length = 0;
      test.renderer.update(50);
      seen.push(test.context.drawImage.mock.calls.length === 2 ? (test.alphaLog[0] ?? 1) : 0);
    }
    expect(seen[0]).toBeCloseTo(0.8, 5);
    expect(seen[1]).toBeCloseTo(0.6, 5);
    expect(seen[2]).toBeCloseTo(0.4, 5);
    expect(seen[3]).toBeCloseTo(0.2, 5);
    expect(seen[4]).toBe(0); // 收工：不再叠帧

    // 溶解全程的不透明度必须单调下降（先画后叠 ⇒ 半透明窗口不会中途透出桌面）。
    test.context.drawImage.mockClear();
    test.renderer.update(42);
    expect(drawnUrls(test)).toEqual(["breath/f00.png"]);
  });

  it("tailBlendMs = 0 ⇒ 退回硬切（只画一次，不写 globalAlpha）", async () => {
    const test = rendererHarness(blendAsset(), { tailBlendMs: 0 });
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();

    test.renderer.playMotion("blink");
    await test.renderer.whenReady();
    test.renderer.update(1);
    test.renderer.update(100);
    test.context.drawImage.mockClear();
    test.alphaLog.length = 0;
    test.renderer.update(100); // 播完
    expect(drawnUrls(test)).toEqual(["breath/f00.png"]);
    expect(test.alphaLog).toEqual([]);
  });

  it("溶解期间沿用动作那一支的窗口轮廓，结束后才收回待机轮廓", async () => {
    const test = rendererHarness(blendAsset());
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();

    test.renderer.playMotion("blink");
    await test.renderer.whenReady();
    test.renderer.update(1);
    expect(test.renderer.consumeSilhouetteDirty()).toBe(true); // 切到 blink，重烘焙
    test.renderer.update(100);
    test.renderer.update(100); // 播完 → 起溶解
    // 此刻画面里还叠着动作末帧：换成待机轮廓会把超出待机的那部分被 SetWindowRgn 切掉。
    expect(test.renderer.consumeSilhouetteDirty()).toBe(false);
    test.renderer.update(250); // 溶解结束
    expect(test.renderer.consumeSilhouetteDirty()).toBe(true);
  });

  it("溶解中途起新动作：溶解立即作废，旧动作末帧不会盖在新动作上", async () => {
    const test = rendererHarness(blendAsset());
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.loadAll();

    test.renderer.playMotion("blink");
    await test.renderer.whenReady();
    test.renderer.update(1);
    test.renderer.update(100);
    test.renderer.update(100); // 起溶解
    test.renderer.update(50); // 溶解中（remaining 0.8）
    test.context.drawImage.mockClear();
    test.renderer.playMotion("blink"); // 重新触发
    test.renderer.update(1);
    expect(drawnUrls(test)).toEqual(["blink/f00.png"]);
  });
});
