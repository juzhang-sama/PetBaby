import { creationApi } from "../creation/api";
import { parseAnimatedImageManifest } from "../runtime/animated-image-manifest";
import { AnimatedImageRenderer } from "../runtime/animated-image-renderer";

export type PixelRuntimeEvidence = {
  readonly renderer: "animated-image-v1";
  readonly neutralPixels: number;
  readonly changedPixels: number;
  readonly manifestSha256: string;
};

export interface PhotoAvatarPreviewHandle {
  readonly evidence: PixelRuntimeEvidence | null;
  destroy(): void;
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

async function manifestSha256(value: unknown): Promise<string> {
  const encoded = new TextEncoder().encode(JSON.stringify(value, null, 2));
  const digest = await crypto.subtle.digest("SHA-256", encoded);
  return [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
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

function canvasPixels(canvas: HTMLCanvasElement): Uint8ClampedArray {
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

function changedPixelCount(before: Uint8ClampedArray, after: Uint8ClampedArray): number {
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
    const manifestHash = await manifestSha256(manifestValue);
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
