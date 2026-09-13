import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { PhotoAvatarSnapshot } from "../creation/api";
import type { FrameSequenceRenderer } from "../runtime/frame-sequence-renderer";
import { mountFrameSequencePreview, type FramePreviewPorts } from "./photo-avatar-frame-preview";
import type { FrameSequenceRuntimeEvidence } from "./photo-avatar-preview-contract";

// node 测试环境里没有这几个浏览器全局；渲染栈要用，补最小的桩。
// （生产代码不为此加注入点 —— 它们是渲染器本来就依赖的平台能力，不是我们的抽象。）
class FakeCanvasElement {
  getContext(_kind?: string): unknown {
    return null;
  }
}

beforeAll(() => {
  vi.stubGlobal("ResizeObserver", class {
    observe(): void {}
    disconnect(): void {}
  });
  vi.stubGlobal("window", {
    requestAnimationFrame: () => 0,
    cancelAnimationFrame: () => undefined,
  });
  vi.stubGlobal("HTMLCanvasElement", FakeCanvasElement);
});
afterAll(() => {
  vi.unstubAllGlobals();
});

const FRAME_COUNT = 5;
const FRAMES = Array.from(
  { length: FRAME_COUNT },
  (_, index) => `frames/idle-combo/f${String(index).padStart(4, "0")}.webp`,
);
const BASE_IMAGE = FRAMES[0]!;

/** 一份**够 `parseFrameSequenceManifest` 通过**的最小 schema 7 manifest。 */
function manifest(): Record<string, unknown> {
  const semantics = Object.fromEntries(
    [
      "idle",
      "look-left",
      "look-right",
      "react-happy",
      "react-curious",
      "carried",
      "landed",
      "sleep",
      "wake",
    ].map((motion) => [motion, "idle-combo"]),
  );
  return {
    schemaVersion: 7,
    renderer: "frame-sequence-v1",
    petId: "pet-a",
    variantId: "combo-loop-v1",
    displayName: "我的猫",
    species: "cat",
    baseImage: BASE_IMAGE,
    defaultAction: "idle-combo",
    anchorPolicy: "fixed",
    actions: [{ actionId: "idle-combo", loop: true, frameDurationMs: 42, frames: FRAMES }],
    semantics,
    files: FRAMES.map((path) => ({ role: "frame", relativePath: path, sha256: "0".repeat(64) })),
  };
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

const MANIFEST_BYTES = new TextEncoder().encode("{\n  \"schemaVersion\": 7\n}\n");

function fakeCanvas(getImageData: () => Uint8ClampedArray): HTMLCanvasElement {
  // `canvasPixels` 会先看 `width`/`height`，再取 `getImageData(...).data` —— 桩要对上这个形状。
  return Object.assign(new FakeCanvasElement(), {
    width: 1,
    height: 1,
    getContext: () => ({ getImageData: () => ({ data: getImageData(), width: 1, height: 1 }) }),
  }) as unknown as HTMLCanvasElement;
}

function previewRoot(): { root: HTMLElement } {
  // 每读一次画布就换一份像素，模拟「确实在动」。
  // ⚠️ 变动的像素数要**超过自检阈值**（20），否则会被判成「没动」——
  // 造两份只有 1~2 个像素不同的数组是测不出真东西的。
  const first = new Uint8ClampedArray(64 * 4);
  const second = new Uint8ClampedArray(64 * 4);
  for (let pixel = 0; pixel < 64; pixel += 1) {
    second.set([255, 255, 255, 255], pixel * 4);
  }
  const pixels = [first, second];
  let reads = 0;
  const canvas = fakeCanvas(() => pixels[Math.min(reads++, pixels.length - 1)]!);
  return { root: rootWith(canvas) };
}

function rootWith(canvas: HTMLCanvasElement): HTMLElement {
  return {
    querySelector: () => canvas,
    getBoundingClientRect: () => ({ width: 480, height: 300 }),
    clientWidth: 480,
    clientHeight: 300,
  } as unknown as HTMLElement;
}

function harness() {
  const fetched: string[] = [];
  const createdUrls: string[] = [];
  const revokedUrls: string[] = [];
  const renderer = {
    resize: vi.fn(),
    load: vi.fn(async () => undefined),
    setVisibility: vi.fn(),
    playMotion: vi.fn(() => ({ cancel: vi.fn() })),
    update: vi.fn(),
    destroy: vi.fn(),
  };
  const snapshot: PhotoAvatarSnapshot = {
    route: "frame-video-v1",
    sessionId: "session-1",
    revision: 1,
    step: "runtimeCheckPending",
    providerJobId: null,
    profile: null,
    attempts: {},
    errorCode: null,
    errorMessage: null,
  };
  const api = {
    photoAvatarStatus: vi.fn(async () => snapshot),
    photoAvatarPreviewManifest: vi.fn(async () => manifest()),
    photoAvatarPreviewFileB64: vi.fn(async (_sessionId: string, _revision: number, path: string) => {
      fetched.push(path);
      return path === "manifest.json"
        ? bytesToBase64(MANIFEST_BYTES)
        : bytesToBase64(new TextEncoder().encode(path));
    }),
    photoAvatarRuntimeCheckPassed: vi.fn(async () => snapshot),
  };
  const ports: FramePreviewPorts = {
    api: api as unknown as FramePreviewPorts["api"],
    createRenderer: () => renderer as unknown as FrameSequenceRenderer,
    createObjectUrl: (bytes) => {
      const url = `blob:${createdUrls.length}:${bytes.length}`;
      createdUrls.push(url);
      return url;
    },
    revokeObjectUrl: (url) => revokedUrls.push(url),
  };
  const { root } = previewRoot();
  return { ports, api, renderer, root, fetched, createdUrls, revokedUrls, snapshot };
}

describe("mountFrameSequencePreview", () => {
  it("播放包里**全部**帧，不做抽样", async () => {
    const h = harness();

    const handle = await mountFrameSequencePreview(h.root, "session-1", h.ports);

    // 帧序列里动作不一定在开头（呼吸/眨眼/摇尾焊在同一支循环），
    // 截前 N 帧就失去意义 —— 所以这里断言「一帧不少」。
    expect(h.fetched.filter((path) => path !== "manifest.json").sort()).toEqual([...FRAMES].sort());
    expect(h.renderer.load).toHaveBeenCalledOnce();
    expect(handle.evidence?.renderer).toBe("frame-sequence-v1");
    // handle 的 evidence 是两条产线共用的宽类型，帧数要缩窄了读。
    expect((handle.evidence as FrameSequenceRuntimeEvidence | null)?.frameCount).toBe(FRAME_COUNT);
    handle.destroy();
  });

  it("用 manifest 的**原始字节**算哈希并回传，而不是重建 JSON", async () => {
    const h = harness();

    const handle = await mountFrameSequencePreview(h.root, "session-1", h.ports);

    // 期望值直接对那串字节算 —— 与 Rust 侧「读文件 → 算 sha256」同一条口径。
    const digest = await crypto.subtle.digest("SHA-256", Uint8Array.from(MANIFEST_BYTES).buffer);
    const expected = Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
    expect(handle.evidence?.manifestSha256).toBe(expected);
    expect(h.api.photoAvatarRuntimeCheckPassed).toHaveBeenCalledWith("session-1", 1, expected);
    handle.destroy();
  });

  it("撤下预览时释放每一张 object URL", async () => {
    const h = harness();

    const handle = await mountFrameSequencePreview(h.root, "session-1", h.ports);
    // baseImage 与首帧是同一条路径 → 去重后只建 FRAME_COUNT 个 URL。
    expect(h.createdUrls).toHaveLength(FRAME_COUNT);
    expect(h.revokedUrls).toEqual([]);

    handle.destroy();

    expect(h.revokedUrls.sort()).toEqual([...h.createdUrls].sort());
    expect(h.renderer.destroy).toHaveBeenCalledOnce();
  });

  it("还没到运行检查阶段就不给挂预览", async () => {
    const h = harness();
    h.api.photoAvatarStatus.mockResolvedValue({ ...h.snapshot, step: "generateMotionSource" });

    await expect(mountFrameSequencePreview(h.root, "session-1", h.ports)).rejects.toThrow(
      "写实分身尚未进入运行检查阶段",
    );
    expect(h.renderer.load).not.toHaveBeenCalled();
  });

  it("画面没动就不许通过运行检查，也不置 previewReady", async () => {
    const h = harness();
    const still = new Uint8ClampedArray([255, 0, 0, 255, 0, 0, 0, 0]);
    const root = rootWith(fakeCanvas(() => still));

    await expect(mountFrameSequencePreview(root, "session-1", h.ports)).rejects.toThrow(
      "写实预览运行检查未检测到动作变化",
    );
    expect(h.api.photoAvatarRuntimeCheckPassed).not.toHaveBeenCalled();
    // 失败也要把 blob 清掉，别把上万个 object URL 留在内存里。
    expect(h.revokedUrls.sort()).toEqual([...h.createdUrls].sort());
  });
});
