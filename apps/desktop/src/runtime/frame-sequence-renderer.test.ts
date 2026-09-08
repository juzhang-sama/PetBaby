import { describe, expect, it, vi } from "vitest";
import { FrameSequenceRenderer } from "./frame-sequence-renderer";
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
    semantics: { idle: "breath", "react-happy": "tail-wag" },
    idleSchedule: {
      entries: [{ actionId: "blink", weight: 1, minIntervalMs: 2500, maxIntervalMs: 2500 }],
    },
    hitBounds: { left: 0.1, top: 0.1, right: 0.9, bottom: 0.9 },
    ...overrides,
  };
}

function rendererHarness(overrides: Partial<FrameSequenceAsset> = {}) {
  const contexts = Array.from({ length: 2 }, () => ({
    clearRect: vi.fn(),
    drawImage: vi.fn(),
    setTransform: vi.fn(),
  }));
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
  });
  renderer.setVisibility(true);
  const asset = frameAsset(overrides);
  return {
    renderer,
    root,
    loadImage,
    random,
    asset,
    // 懒加载后 load() 只阻塞默认动作；测试需要"全部动作就绪"时用它。
    load: async () => {
      await renderer.load(asset);
      await renderer.whenReady();
    },
    context: contexts[0]!,
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
    await test.load();
    expect(test.loadImage).toHaveBeenCalledTimes(1 + 3 + 2 + 2);
    expect(test.root.replaceChildren).toHaveBeenCalledWith(test.displayCanvas, test.hitCanvas);
  });

  it("loads only the default action eagerly and defers the rest to background", async () => {
    const test = rendererHarness();
    // 只 await load()：默认动作 breath（3 帧）+ baseImage 已加载，可立即渲染首帧。
    await test.renderer.load(test.asset);
    expect(test.loadImage).toHaveBeenCalledWith(test.asset.baseImageUrl);
    expect(test.loadImage).toHaveBeenCalledWith("breath/f00.png");
    expect(test.root.replaceChildren).toHaveBeenCalledWith(test.displayCanvas, test.hitCanvas);
    // 后台加载通过 whenReady 收敛到全量（含 blink + tail-wag）。
    await test.renderer.whenReady();
    expect(test.loadImage).toHaveBeenCalledTimes(1 + 3 + 2 + 2);
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
    await test.load();
    const handle = test.renderer.playMotion("react-happy");
    test.context.drawImage.mockClear();
    test.renderer.update(120);
    const tailFrame0 = test.context.drawImage.mock.calls[0]![0];
    test.renderer.update(120);
    const tailFrame1 = test.context.drawImage.mock.calls.at(-1)![0];
    expect(tailFrame0).not.toBe(tailFrame1);
    test.renderer.update(120);
    test.context.drawImage.mockClear();
    test.renderer.update(180);
    const afterCycle = test.context.drawImage.mock.calls[0]![0];
    expect(afterCycle).not.toBe(tailFrame1);
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
    await test.load();
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
    await test.load();
    test.context.drawImage.mockClear();
    test.renderer.update(2500);
    test.renderer.update(120);
    const tailFrame0 = test.context.drawImage.mock.calls.at(-1)![0];
    expect(tailFrame0).toBeDefined();
    test.renderer.update(120);
    const tailFrame1 = test.context.drawImage.mock.calls.at(-1)![0];
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
    await test.load();
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
    await test.load();
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
    await test.load();
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
    await test.load();
    test.context.drawImage.mockClear();
    test.renderer.update(2500); // armed at phase 340
    test.renderer.update(100); // 2600
    test.renderer.update(100); // 2700 boundary -> blink frame 0
    test.renderer.update(150); // blink frame 1
    test.renderer.update(150); // blink done -> resume breath at phase 0
    const resumed = test.context.drawImage.mock.calls.at(-1)![0];
    expect(resumed).toBe(test.imageByUrl.get("breath/f00.png"));
  });

  // 窗口裁剪区（SetWindowRgn）按 hit surface 的 alpha 生成。若 hit surface 固定在
  // baseImage（f0000），动作期间身体形变超出首帧轮廓的像素就会被窗口裁掉。
  describe("窗口裁剪轮廓（hit surface）", () => {
    it("用当前动作所有帧的并集轮廓，而不是静态首帧", async () => {
      const test = rendererHarness();
      test.renderer.resize({ width: 400, height: 500, dpr: 2 });
      await test.load();

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
      await test.load();
      expect(test.renderer.consumeSilhouetteDirty()).toBe(true);

      test.renderer.playMotion("react-happy"); // semantics -> tail-wag
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
      await test.load();
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
      await test.load();
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
    await test.load();

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
    await test.load();

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

    // 再跨过总时长 → 放下段播完，回归默认动作 breath。
    test.renderer.update(42);
    const backToIdle = test.context.drawImage.mock.calls.at(-1)![0];
    expect(backToIdle).toBe(frameAt(test, "breath/f0000.webp"));
  });

  it("拎起中途松手：不跳到放下段，从当前帧自然续播", async () => {
    const test = rendererHarness(grabAsset([35, 63]));
    test.renderer.resize({ width: 400, height: 500, dpr: 2 });
    await test.load();

    test.renderer.playMotion("carried", { loop: true });
    for (let k = 1; k <= 10; k += 1) test.renderer.update(42); // 停在第 10 帧（< lo）
    test.renderer.playMotion("landed");
    test.renderer.update(42);
    const resumed = test.context.drawImage.mock.calls.at(-1)![0];
    expect(resumed).toBe(frameAt(test, "grab/f0011.webp"));
    expect(resumed).not.toBe(frameAt(test, "grab/f0064.webp"));
  });
});
