import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import { loadFrameSequenceAsset } from "./frame-sequence-asset-loader";
import { parseFrameSequenceManifest } from "./frame-sequence-manifest";
import { FrameSequenceRenderer } from "./frame-sequence-renderer";
import type { PetRenderAsset } from "./pet-renderer";

type FrameSequenceAsset = Extract<PetRenderAsset, { kind: "frame-sequence" }>;

// 真实组合循环包：毛砌墙 v3 呼吸+眨眼+摇尾（schemaVersion 7，单 action 循环，
// 由 scripts/poc_组合循环打包.py 产出，源 09-组合循环/01-帧序列）
const MANIFEST_URL = new URL(
  "../../../../output/宠物动作-毛砌墙-v3-2026-08-31/09-组合循环/10-运行时包/manifest.json",
  import.meta.url,
);
const MANIFEST_ROOT = new URL("./", MANIFEST_URL);

// 组合循环参数：与 scripts/poc_组合循环打包.py 同步（287 帧 = 12.05s 循环）。
// 换序列时只改这里，下面用例都跟着走（别在用例里再硬编码帧数/时长）。
const COMBO_FRAMES = 287;
const COMBO_MS = COMBO_FRAMES * 42; // 12054ms

describe("组合循环包：呼吸 + 眨眼 + 摇尾（单视频循环，无 idleSchedule）", () => {
  const raw = JSON.parse(readFileSync(MANIFEST_URL, "utf8"));

  it("manifest 通过真实 parseFrameSequenceManifest 解析", () => {
    const parsed = parseFrameSequenceManifest(raw);
    expect(parsed.schemaVersion).toBe(7);
    expect(parsed.defaultAction).toBe("idle-combo");
    expect(parsed.actions.map((a) => a.actionId)).toEqual(["idle-combo"]);
    expect(parsed.actions[0]!.loop).toBe(true);
    // 单视频循环：呼吸/眨眼/摇尾都焊在循环里，无偶发动作调度
    expect(parsed.idleSchedule).toBeUndefined();
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

  it(`组合循环 = ${COMBO_FRAMES} 帧 × 42ms = ${COMBO_MS}ms，loop=true`, () => {
    const parsed = parseFrameSequenceManifest(raw);
    const combo = parsed.actions.find((a) => a.actionId === "idle-combo")!;
    expect(combo.loop).toBe(true);
    expect(combo.frames).toHaveLength(COMBO_FRAMES);
    expect(combo.frameDurationMs).toBe(42);
    expect(combo.frames.length * combo.frameDurationMs).toBe(COMBO_MS);
  });

  it("渲染器：单 action 循环，播完一整圈回绕到首帧", async () => {
    const parsed = parseFrameSequenceManifest(raw);
    const asset = await loadFrameSequenceAsset(
      parsed.petId,
      parsed,
      (petId, path) => `/fake/${petId}/${path}`,
    ) as FrameSequenceAsset;

    // 复用合并包测试的注入式 harness：真实 manifest 结构 + mock 图像
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
    });
    renderer.setVisibility(true);
    renderer.resize({ width: 400, height: 500, dpr: 2 });
    await renderer.load(asset);

    // 初始：循环首帧 f0000
    const petId = parsed.petId;
    const frameUrl = (i: number) => `/fake/${petId}/frames/idle-combo/f${String(i).padStart(4, "0")}.png`;
    expect(contexts[0]!.drawImage.mock.calls[0]![0]).toBe(imageByUrl.get(frameUrl(0)));

    // 推进 42ms → 第 2 帧 f0001
    renderer.update(42);
    expect(contexts[0]!.drawImage.mock.calls.at(-1)![0]).toBe(imageByUrl.get(frameUrl(1)));

    // 推进到正好一整圈（12054ms）→ 回绕到 f0000
    renderer.update(COMBO_MS - 42);
    expect(contexts[0]!.drawImage.mock.calls.at(-1)![0]).toBe(imageByUrl.get(frameUrl(0)));

    // 再推 42ms → f0001（循环继续，不是停在结尾）
    renderer.update(42);
    expect(contexts[0]!.drawImage.mock.calls.at(-1)![0]).toBe(imageByUrl.get(frameUrl(1)));

    renderer.destroy();
  });
});
