import { creationApi } from "../creation/api";
import { parseAnimatedImageManifest } from "../runtime/animated-image-manifest";
import { AnimatedImageRenderer } from "../runtime/animated-image-renderer";
import type {
  PhotoAvatarPreviewHandle,
  PixelRuntimeEvidence,
} from "./photo-avatar-preview-contract";

// 预览契约（handle / evidence）由两条产线共用，定义搬去了
// `photo-avatar-preview-contract.ts`；这里 re-export 保持既有 import 不变。
export type {
  PhotoAvatarPreviewEvidence,
  PixelRuntimeEvidence,
  PhotoAvatarPreviewHandle,
} from "./photo-avatar-preview-contract";

// 下面这几个小工具（base64 / sha256 / 画布取像素 / 视野尺寸）是**两条产线共用**的，
// 所以导出给 `photo-avatar-frame-preview.ts` 用 —— 与其再造一份，不如让它住在这里。
export function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes).buffer);
  return Array.from(new Uint8Array(digest), (value) => value.toString(16).padStart(2, "0")).join("");
}

const HIDDEN_PREVIEW_WIDTH = 480;
const HIDDEN_PREVIEW_HEIGHT = 300;

export function photoAvatarPreviewViewport(
  root: HTMLElement,
  devicePixelRatio = window.devicePixelRatio || 1,
): { width: number; height: number; dpr: number } {
  const bounds = root.getBoundingClientRect();
  const measuredWidth = Math.round(root.clientWidth || bounds.width);
  const measuredHeight = Math.round(root.clientHeight || bounds.height);
  return {
    width: measuredWidth > 0 ? measuredWidth : HIDDEN_PREVIEW_WIDTH,
    height: measuredHeight > 0 ? measuredHeight : HIDDEN_PREVIEW_HEIGHT,
    dpr: Math.max(1, devicePixelRatio),
  };
}

export function canvasPixels(canvas: HTMLCanvasElement): Uint8ClampedArray {
  const context = canvas.getContext("2d");
  if (context === null || canvas.width <= 0 || canvas.height <= 0) return new Uint8ClampedArray();
  return context.getImageData(0, 0, canvas.width, canvas.height).data;
}

function visiblePixelCount(pixels: Uint8ClampedArray): number {
  let count = 0;
  for (let index = 3; index < pixels.length; index += 4) {
    if (pixels[index] !== 0) count += 1;
  }
  return count;
}

export function changedPixelCount(before: Uint8ClampedArray, after: Uint8ClampedArray): number {
  if (before.length === 0 || before.length !== after.length) return 0;
  let count = 0;
  for (let index = 0; index < before.length; index += 4) {
    const difference = Math.abs(before[index]! - after[index]!)
      + Math.abs(before[index + 1]! - after[index + 1]!)
      + Math.abs(before[index + 2]! - after[index + 2]!)
      + Math.abs(before[index + 3]! - after[index + 3]!);
    if (difference > 8) count += 1;
  }
  return count;
}

export async function mountPhotoAvatarPreview(
  root: HTMLElement,
  sessionId: string,
): Promise<PhotoAvatarPreviewHandle> {
  const snapshot = await creationApi.photoAvatarStatus(sessionId);
  if (snapshot === null || (snapshot.step !== "runtimeCheckPending" && snapshot.step !== "previewReady")) {
    throw new Error("像素分身尚未进入运行检查阶段");
  }
  const manifestValue = await creationApi.photoAvatarPreviewManifest(sessionId, snapshot.revision);
  const manifest = parseAnimatedImageManifest(manifestValue);
  const [imageBytes, motionBytes] = await Promise.all([
    creationApi.photoAvatarPreviewFileB64(sessionId, snapshot.revision, manifest.image).then(decodeBase64),
    creationApi.photoAvatarPreviewFileB64(sessionId, snapshot.revision, manifest.motionProfile).then(decodeBase64),
  ]);
  const motionProfile = JSON.parse(new TextDecoder().decode(motionBytes));
  const imageUrl = URL.createObjectURL(new Blob([Uint8Array.from(imageBytes).buffer], { type: "image/png" }));
  const renderer = new AnimatedImageRenderer(root);
  let animationFrame = 0;
  let destroyed = false;
  let previousTime = performance.now();
  let evidence: PixelRuntimeEvidence | null = null;
  const resize = () => renderer.resize(photoAvatarPreviewViewport(root));
  const resizeObserver = new ResizeObserver(resize);
  try {
    resizeObserver.observe(root);
    renderer.resize(photoAvatarPreviewViewport(root));
    await renderer.load({ kind: "animated-image", imageUrl, motionProfile });
    renderer.setVisibility(true);
    const idle = renderer.playMotion("idle", { loop: true, priority: 10 });
    const canvas = root.querySelector("canvas");
    if (!(canvas instanceof HTMLCanvasElement)) throw new Error("像素预览画布不可用");
    renderer.update(0);
    const neutral = canvasPixels(canvas);
    renderer.update(700);
    const moved = canvasPixels(canvas);
    const neutralPixels = visiblePixelCount(neutral);
    const changedPixels = changedPixelCount(neutral, moved);
    if (neutralPixels <= 0 || changedPixels <= 20) {
      throw new Error("像素预览运行检查未检测到有效图像或动作变化");
    }
    const manifestHash = await sha256Hex(
      // ⚠️ 对**磁盘上的原始字节**算，不是拿解析后的 JSON 重新 `JSON.stringify` 再算 ——
      // 后者要跟服务侧（Python `json.dumps(indent=2, ensure_ascii=False)`）写出来的字节
      // 逐字节相等，是个**没人验证过的跨语言约定**。多读一次文件换掉这个假设：
      // 后端拿的就是这份字节的哈希，算错等于这条链走不到「预览就绪」。
      decodeBase64(await creationApi.photoAvatarPreviewFileB64(sessionId, snapshot.revision, "manifest.json")),
    );
    evidence = { renderer: "animated-image-v1", neutralPixels, changedPixels, manifestSha256: manifestHash };
    if (snapshot.step === "runtimeCheckPending") {
      await creationApi.photoAvatarRuntimeCheckPassed(sessionId, snapshot.revision, manifestHash);
    }
    const frame = (now: number) => {
      if (destroyed) return;
      renderer.update(Math.min(100, Math.max(0, now - previousTime)));
      previousTime = now;
      animationFrame = window.requestAnimationFrame(frame);
    };
    animationFrame = window.requestAnimationFrame(frame);
    return {
      get evidence() { return evidence; },
      destroy: () => {
        if (destroyed) return;
        destroyed = true;
        idle.cancel();
        window.cancelAnimationFrame(animationFrame);
        resizeObserver.disconnect();
        renderer.destroy();
        URL.revokeObjectURL(imageUrl);
      },
    };
  } catch (error) {
    destroyed = true;
    window.cancelAnimationFrame(animationFrame);
    resizeObserver.disconnect();
    renderer.destroy();
    URL.revokeObjectURL(imageUrl);
    throw error;
  }
}
