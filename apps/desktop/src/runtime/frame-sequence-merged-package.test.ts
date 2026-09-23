import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import { loadFrameSequenceAsset } from "./frame-sequence-asset-loader";
import { parseFrameSequenceManifest } from "./frame-sequence-manifest";
import { FrameSequenceRenderer } from "./frame-sequence-renderer";
import type { PetRenderAsset } from "./pet-renderer";

type FrameSequenceAsset = Extract<PetRenderAsset, { kind: "frame-sequence" }>;

// 真实合并包：毛砌墙 v3 呼吸+眨眼（schemaVersion 7，由 scripts/poc_合并动作.py 产出）
const MANIFEST_URL = new URL(
  "../../../../output/宠物动作-毛砌墙-v3-2026-08-31/07-合并-呼吸眨眼/manifest.json",
  import.meta.url,
);
const MANIFEST_ROOT = new URL("./", MANIFEST_URL);

// 眨眼序列参数：与 output/.../06-眨眼/06-blend-12帧-504ms 同步。
// 换序列时只改这里，下面两个用例都会跟着走（别在用例里再硬编码帧数/时长）。
const BLINK_FRAMES = 12;
const BLINK_MS = BLINK_FRAMES * 42; // 504ms

describe("合并包：呼吸 + 眨眼（真实资产）", () => {
  const raw = JSON.parse(readFileSync(MANIFEST_URL, "utf8"));

  it("manifest 通过真实 parseFrameSequenceManifest 解析", () => {
    const parsed = parseFrameSequenceManifest(raw);
    expect(parsed.schemaVersion).toBe(7);
    expect(parsed.defaultAction).toBe("breath");
    expect(parsed.actions.map((a) => a.actionId)).toEqual(["breath", "blink"]);
    expect(parsed.idleSchedule?.alignToDefaultLoop).toBe(true);
  });

  it("manifest 声明的每个文件都存在且 sha256 一致", () => {
    const parsed = parseFrameSequenceManifest(raw);
    expect(parsed.files.length).toBeGreaterThan(0);
    for (const file of parsed.files) {
      const url = new URL(file.relativePath, MANIFEST_ROOT);
      const bytes = readFileSync(url);
      const actual = createHash("sha256").update(bytes).digest("hex");
      expect(actual, file.relativePath).toBe(file.sha256);
    }
  });

  it(`呼吸循环 = 60 帧 × 42ms = 2520ms，眨眼 = ${BLINK_FRAMES} 帧 × 42ms = ${BLINK_MS}ms`, () => {
    const parsed = parseFrameSequenceManifest(raw);
    const breath = parsed.actions.find((a) => a.actionId === "breath")!;
    const blink = parsed.actions.find((a) => a.actionId === "blink")!;
    expect(breath.loop).toBe(true);
    expect(breath.frames).toHaveLength(60);
    expect(breath.frameDurationMs).toBe(42);
    expect(blink.loop).toBe(false);
    expect(blink.frames).toHaveLength(BLINK_FRAMES);
    expect(blink.frameDurationMs).toBe(42);
    expect(blink.frames.length * blink.frameDurationMs).toBe(BLINK_MS);
  });

  it("渲染器：眨眼对齐呼吸循环边界触发，播完回到呼吸 phase 0", async () => {
    const parsed = parseFrameSequenceManifest(raw);
    const asset = await loadFrameSequenceAsset(
      parsed.petId,
      parsed,
      (petId, path) => `/fake/${petId}/${path}`,
    );

    // 复用渲染器测试的注入式 harness：真实 manifest 结构 + mock 图像
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
    let canvasIndex = 0;
    const root = { replaceChildren: vi.fn() } as unknown as HTMLElement;
    const imageByUrl = new Map<string, CanvasImageSource & { width: number; height: number }>();
    const loadImage = vi.fn(async (url: string) => {
      let image = imageByUrl.get(url);
      if (!image) {
        image = { width: 588, height: 588 } as CanvasImageSource & { width: number; height: number };
        imageByUrl.set(url, image);
      }
      return image;
    });
    const renderer = new FrameSequenceRenderer(root, {
      createCanvas: () => canvases[canvasIndex++]!,
      loadImage,
      random: vi.fn(() => 0.5), // 固定随机，min=2500 → next=2500+0.5*(3500)=4250
    });
    renderer.setVisibility(true);
    renderer.resize({ width: 400, height: 500, dpr: 2 });
    await renderer.load(asset);
    // 懒加载：blink 是非默认动作，后台加载，等它就绪再断言触发行为。
    await renderer.whenReady();

    // 初始：呼吸 phase 0
    const firstFrame = contexts[0]!.drawImage.mock.calls[0]![0];
    expect(firstFrame).toBe(imageByUrl.get("/fake/宠物动作-毛砌墙-v3-2026-08-31/frames/breath/f0000.png"));

    contexts[0]!.drawImage.mockClear();
    // 走到 4250ms：间隔已到（>=4250），但 4250 % 2520 = 1730 ≠ 0 → 对齐等待，
    // 期间呼吸循环继续推进到相位 1730ms → 帧 index = floor(1730/42) = 41。
    // （对齐等待是"挂起一次性动作、等循环边界"，不是"冻结呼吸"。）
    renderer.update(4250);
    expect(contexts[0]!.drawImage.mock.calls.at(-1)![0])
      .toBe(imageByUrl.get("/fake/宠物动作-毛砌墙-v3-2026-08-31/frames/breath/f0041.png"));
    // 4250→5040（循环边界）：眨眼触发，第一帧 = blink f0000
    renderer.update(5040 - 4250);
    await renderer.whenReady();
    expect(contexts[0]!.drawImage.mock.calls.at(-1)![0])
      .toBe(imageByUrl.get("/fake/宠物动作-毛砌墙-v3-2026-08-31/frames/blink/f0000.png"));
    // 眨眼推进到 168ms（第 5 帧，index = floor(168/42) = 4）→ 仍在 blink 中
    renderer.update(168);
    expect(contexts[0]!.drawImage.mock.calls.at(-1)![0])
      .toBe(imageByUrl.get("/fake/宠物动作-毛砌墙-v3-2026-08-31/frames/blink/f0004.png"));
    // 眨眼播完（补齐到 BLINK_MS）→ 回到呼吸 phase 0。
    // 动作末帧会叠在待机帧上做收尾溶解 ⇒ 本 tick 的**第一次** drawImage 才是当前动作帧。
    contexts[0]!.drawImage.mockClear();
    renderer.update(BLINK_MS - 168);
    expect(contexts[0]!.drawImage.mock.calls[0]![0])
      .toBe(imageByUrl.get("/fake/宠物动作-毛砌墙-v3-2026-08-31/frames/breath/f0000.png"));

    renderer.destroy();
  });
});
