import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { loadFrameSequenceAsset } from "./frame-sequence-asset-loader";
import { parseFrameSequenceManifest } from "./frame-sequence-manifest";
import { FrameSequenceRenderer } from "./frame-sequence-renderer";

// 互动语义：点击身体产生的"好奇"intent，必须落到真实存在的偶发动作上，
// 否则 playMotion 会 fallback 到 idle-combo，用户点击猫毫无反应（2026-09-04 发现的缺口）。
// 双击（react-happy）故意不映射——将来做"弹对话框"引出属性功能，不是动作反馈。
const INTERACTION_SEMANTICS: Record<string, string> = {
  "react-curious": "lick", // 点身体 → 舔毛（被戳了舔一下）
};

const PET_MANIFESTS = [
  "04-warm-brown-tabby",
  "05-silver-tabby",
] as const;

describe("内置宠物互动语义", () => {
  for (const petId of PET_MANIFESTS) {
    it(`${petId} 的点击/双击语义映射到真实动作`, () => {
      const raw = JSON.parse(
        readFileSync(
          new URL(`../../public/builtin-pets/${petId}/manifest.json`, import.meta.url),
          "utf8",
        ),
      );
      const parsed = parseFrameSequenceManifest(raw);
      const actionIds = parsed.actions.map((a) => a.actionId);
      for (const [motion, actionId] of Object.entries(INTERACTION_SEMANTICS)) {
        expect(parsed.semantics[motion], `${motion} 语义`).toBe(actionId);
        expect(actionIds, `${motion} → ${actionId} 目标动作`).toContain(actionId);
      }
    });
  }

  it("05-silver-tabby 拖拽语义（carried/landed）映射到带 holdRange 的 grab-release", () => {
    const petId = "05-silver-tabby";
    const raw = JSON.parse(
      readFileSync(
        new URL(`../../public/builtin-pets/${petId}/manifest.json`, import.meta.url),
        "utf8",
      ),
    );
    const parsed = parseFrameSequenceManifest(raw);
    const grab = parsed.actions.find((a) => a.actionId === "grab-release");
    expect(grab).toBeDefined();
    expect(parsed.semantics["carried"]).toBe("grab-release");
    expect(parsed.semantics["landed"]).toBe("grab-release");
    // holdRange 必须落在帧区间内，且渲染器据此做"悬空保持"循环。
    const [lo, hi] = grab!.holdRange ?? [-1, -1];
    expect(lo).toBeGreaterThanOrEqual(0);
    expect(hi).toBeLessThan(grab!.frames.length);
    expect(hi).toBeGreaterThan(lo);
  });

  it("playMotion 触发映射后的偶发动作，播完回归待机", async () => {
    const petId = "04-warm-brown-tabby";
    const raw = JSON.parse(
      readFileSync(
        new URL(`../../public/builtin-pets/${petId}/manifest.json`, import.meta.url),
        "utf8",
      ),
    );
    const parsed = parseFrameSequenceManifest(raw);
    const asset = await loadFrameSequenceAsset(
      parsed.petId,
      parsed,
      (_, path) => `/fake/${path}`,
    );

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
      random: vi.fn(() => 0.5),
    });
    renderer.setVisibility(true);
    renderer.resize({ width: 400, height: 500, dpr: 2 });
    await renderer.load(asset);
    await renderer.whenReady();

    // 点身体 → react-curious → lick 首帧
    renderer.playMotion("react-curious");
    contexts[0]!.drawImage.mockClear();
    renderer.update(1);
    const lickFrame = contexts[0]!.drawImage.mock.calls[0]![0];
    expect(lickFrame).toBe(imageByUrl.get("/fake/frames/lick/f0000.webp"));

    // 播完 lick（119 帧 × 42ms）→ 回归 idle-combo
    renderer.update(119 * 42 - 1);
    contexts[0]!.drawImage.mockClear();
    renderer.update(1);
    const backToIdle = contexts[0]!.drawImage.mock.calls[0]![0];
    expect(backToIdle).toBe(imageByUrl.get("/fake/frames/idle-combo/f0000.webp"));

    renderer.destroy();
  });
});
