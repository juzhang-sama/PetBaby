import { creationApi } from "../creation/api";
import {
  loadLive2DAsset,
  photoAvatarManifestSha256,
  type Live2DAssetTransport,
} from "../runtime-assets/live2d-asset-loader";
import {
  parseCatSpatialManifest,
  type RuntimeAssetManifestV5,
} from "../runtime-assets/cat-spatial-manifest";
import { Live2DRenderer } from "../runtime-live2d/live2d-renderer";
import { CAT_MOTION_SET_V1, type CatMotionNameV1 } from "../runtime-live2d/cat-motion-contract";
import {
  advanceCatMotionEvidenceTime,
  assertCompleteCatMotionEvidence,
  CAT_MOTION_EVIDENCE_TIMING,
  renderCatMotionEvidencePhase,
  type CatMotionFrameEvidenceV1,
  type CatMotionInterruptionEvidenceV1,
  type CatMotionInterruptionPhase,
  type CatMotionRuntimeEvidenceV1,
} from "../runtime-live2d/cat-motion-evidence";
import type { PetMotionHandle, PetRenderer } from "../runtime/pet-renderer";

export interface PhotoAvatarPreviewHandle {
  readonly evidence: CatMotionRuntimeEvidenceV1 | null;
  destroy(): void;
}

interface PreviewResizeObserver {
  observe(target: Element): void;
  disconnect(): void;
}

interface PreviewManifestResult {
  revision: number;
  step: "runtimeCheckPending" | "previewReady";
  manifest: unknown;
}

type PhotoAvatarLive2DRenderer = PetRenderer & {
  state(): { status: "unloaded" | "loading" | "ready" | "context-lost" | "destroyed"; visible: boolean };
  supportsCatMotionV1(): boolean;
  playCatMotion: NonNullable<PetRenderer["playCatMotion"]>;
};

export interface PhotoAvatarPreviewPorts {
  loadLive2DAsset: typeof loadLive2DAsset;
  createLive2DRenderer(canvas: HTMLCanvasElement): PhotoAvatarLive2DRenderer;
  previewManifest(sessionId: string): Promise<PreviewManifestResult>;
  previewTransport(sessionId: string, revision: number, manifest: unknown): Live2DAssetTransport;
  runtimeCheckPassed(sessionId: string, revision: number, manifestSha256: string): Promise<unknown>;
  manifestSha256(manifest: RuntimeAssetManifestV5): Promise<string>;
  createCanvas(): HTMLCanvasElement;
  renderedPixelCount(canvas: HTMLCanvasElement): number;
  renderedFrame(canvas: HTMLCanvasElement): Uint8Array;
  frameSha256(frame: Uint8Array): Promise<string>;
  requestAnimationFrame(callback: FrameRequestCallback): number;
  cancelAnimationFrame(id: number): void;
  createResizeObserver(callback: ResizeObserverCallback): PreviewResizeObserver;
  devicePixelRatio(): number;
  prefersReducedMotion(): boolean;
  onReducedMotionChange(listener: (reduced: boolean) => void): () => void;
}

function decodeBase64(encoded: string): Uint8Array {
  const binary = atob(encoded);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

export function photoAvatarRenderedFrame(canvas: HTMLCanvasElement): Uint8Array {
  const gl = canvas.getContext("webgl2") ?? canvas.getContext("webgl");
  if (gl === null || gl.isContextLost()) return new Uint8Array();
  const width = gl.drawingBufferWidth;
  const height = gl.drawingBufferHeight;
  if (width <= 0 || height <= 0) return new Uint8Array();
  const pixels = new Uint8Array(width * height * 4);
  gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
  return pixels;
}

export function photoAvatarRenderedPixelCount(canvas: HTMLCanvasElement): number {
  const pixels = photoAvatarRenderedFrame(canvas);
  let count = 0;
  for (let index = 3; index < pixels.length; index += 4) {
    if (pixels[index] !== 0) count += 1;
  }
  return count;
}

export function photoAvatarFramePixelDifference(before: Uint8Array, after: Uint8Array): number {
  if (before.length === 0 || before.length !== after.length || before.length % 4 !== 0) return 0;
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

export async function photoAvatarFrameSha256(frame: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", Uint8Array.from(frame));
  return Array.from(new Uint8Array(digest), (value) => value.toString(16).padStart(2, "0")).join("");
}

function renderedFramePixelCount(frame: Uint8Array): number {
  if (frame.length === 0 || frame.length % 4 !== 0) return 0;
  let count = 0;
  for (let index = 3; index < frame.length; index += 4) {
    if (frame[index] !== 0) count += 1;
  }
  return count;
}

const browserPorts: PhotoAvatarPreviewPorts = {
  loadLive2DAsset,
  createLive2DRenderer: (canvas) => new Live2DRenderer(canvas),
  previewManifest: async (sessionId) => {
    const snapshot = await creationApi.photoAvatarStatus(sessionId);
    if (snapshot === null) {
      throw new Error("photo avatar run is unavailable for preview");
    }
    if (snapshot.step !== "runtimeCheckPending" && snapshot.step !== "previewReady") {
      throw new Error(`photo avatar preview is not ready for runtime check: ${snapshot.step}`);
    }
    return {
      revision: snapshot.revision,
      step: snapshot.step,
      manifest: await creationApi.photoAvatarPreviewManifest(sessionId, snapshot.revision),
    };
  },
  previewTransport: (sessionId, revision, manifest) => ({
    // Bind the loader's expected and served manifests to the same command result.
    readManifest: async () => manifest,
    readFile: async (_petId, relativePath) => decodeBase64(
      await creationApi.photoAvatarPreviewFileB64(sessionId, revision, relativePath),
    ),
  }),
  runtimeCheckPassed: (sessionId, revision, manifestSha256) =>
    creationApi.photoAvatarRuntimeCheckPassed(sessionId, revision, manifestSha256),
  manifestSha256: photoAvatarManifestSha256,
  createCanvas: () => document.createElement("canvas"),
  renderedPixelCount: photoAvatarRenderedPixelCount,
  renderedFrame: photoAvatarRenderedFrame,
  frameSha256: photoAvatarFrameSha256,
  requestAnimationFrame: (callback) => window.requestAnimationFrame(callback),
  cancelAnimationFrame: (id) => window.cancelAnimationFrame(id),
  createResizeObserver: (callback) => new ResizeObserver(callback),
  devicePixelRatio: () => Math.max(1, window.devicePixelRatio || 1),
  prefersReducedMotion: () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  onReducedMotionChange: (listener) => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const handler = (event: MediaQueryListEvent) => listener(event.matches);
    query.addEventListener("change", handler);
    return () => query.removeEventListener("change", handler);
  },
};

function requirePhotoAvatarManifest(value: unknown): RuntimeAssetManifestV5 {
  const manifest = parseCatSpatialManifest(value);
  if (manifest.schemaVersion !== 5 || manifest.renderer !== "cat-spatial-live2d-v1") {
    throw new Error("photo avatar preview requires a spatial v5 Live2D manifest");
  }
  return manifest;
}

function viewport(root: HTMLElement, dpr: number): { width: number; height: number; dpr: number } {
  const bounds = root.getBoundingClientRect();
  return {
    width: Math.max(1, Math.round(bounds.width)),
    height: Math.max(1, Math.round(bounds.height)),
    dpr: Math.max(1, dpr),
  };
}

function assertRendererReady(renderer: PhotoAvatarLive2DRenderer): void {
  const status = renderer.state().status;
  if (status !== "ready") throw new Error(`photo avatar renderer is not ready: ${status}`);
}

function assertSpatialRenderer(renderer: PhotoAvatarLive2DRenderer): void {
  if (!renderer.supportsCatMotionV1()) {
    throw new Error("photo avatar preview requires a spatial Live2D renderer");
  }
}

async function inspectMotion(
  renderer: PhotoAvatarLive2DRenderer,
  motion: CatMotionNameV1,
  renderedFrame: () => Uint8Array,
  frameSha256: (frame: Uint8Array) => Promise<string>,
): Promise<CatMotionFrameEvidenceV1[]> {
  assertRendererReady(renderer);
  const neutral = renderedFrame();
  if (renderedFramePixelCount(neutral) <= 0) {
    throw new Error(`photo avatar motion produced an empty neutral framebuffer: ${motion}`);
  }
  const neutralEvidence: CatMotionFrameEvidenceV1 = {
    motion,
    phase: "neutral",
    framebufferNonEmpty: true,
    changedPixelCount: 0,
    sha256: await frameSha256(neutral),
    renderer: "cat-spatial-live2d-v1",
  };
  if (motion === "breathing") renderer.setCatAutomationMode?.("idle");
  if (motion === "pointer-focus") renderer.setLookTarget({ x: 0.65, y: 0.35 });
  let finished = false;
  const action = renderer.playCatMotion(motion, {
    priority: 80,
    fadeInMs: 120,
    fadeOutMs: 180,
  }, () => { finished = true; });
  try {
    const timing = CAT_MOTION_EVIDENCE_TIMING[motion];
    advanceCatMotionEvidenceTime((deltaMs) => {
      renderer.update(deltaMs);
      assertRendererReady(renderer);
    }, timing.peakMs);
    const peak = renderedFrame();
    const peakPixelDifference = photoAvatarFramePixelDifference(neutral, peak);
    if (renderedFramePixelCount(peak) <= 0 || peakPixelDifference <= 20) {
      action.cancel();
      throw new Error(`photo avatar motion has no visible pixel change at peak: ${motion}`);
    }
    const peakEvidence: CatMotionFrameEvidenceV1 = {
      motion,
      phase: "peak",
      framebufferNonEmpty: true,
      changedPixelCount: peakPixelDifference,
      sha256: await frameSha256(peak),
      renderer: "cat-spatial-live2d-v1",
    };

    advanceCatMotionEvidenceTime((deltaMs) => {
      renderer.update(deltaMs);
      assertRendererReady(renderer);
    }, Math.max(0, timing.fallbackMs - timing.peakMs));
    action.cancel();
    assertRendererReady(renderer);

    const idle = renderer.playCatMotion("breathing", {
      loop: true,
      priority: 10,
      fadeInMs: 180,
      fadeOutMs: 140,
    });
    advanceCatMotionEvidenceTime((deltaMs) => {
      renderer.update(deltaMs);
      assertRendererReady(renderer);
    }, 240);
    const fallback = renderedFrame();
    assertRendererReady(renderer);
    idle.cancel();
    assertRendererReady(renderer);

    const fallbackPixelDifference = photoAvatarFramePixelDifference(peak, fallback);
    if (renderedFramePixelCount(fallback) <= 0 || fallbackPixelDifference <= 20) {
      throw new Error(`photo avatar motion has no visible pixel change at fallback: ${motion}`);
    }
    const fallbackEvidence: CatMotionFrameEvidenceV1 = {
      motion,
      phase: "fallback",
      framebufferNonEmpty: true,
      changedPixelCount: fallbackPixelDifference,
      sha256: await frameSha256(fallback),
      renderer: "cat-spatial-live2d-v1",
    };
    if (
      motion === "half-stand-stretch"
      && photoAvatarFramePixelDifference(neutral, fallback) <= 20
    ) {
      throw new Error("half-stand-stretch neutral, peak, and fallback must be visibly distinct");
    }

    if (!finished && timing.fallbackMs >= timing.durationMs) {
      throw new Error(`photo avatar motion did not finish at its authored boundary: ${motion}`);
    }
    return [neutralEvidence, peakEvidence, fallbackEvidence];
  } finally {
    renderer.setLookTarget(null);
    if (motion === "breathing") renderer.setCatAutomationMode?.("paused");
  }
}

async function inspectInterruption(
  renderer: PhotoAvatarLive2DRenderer,
  motion: "half-stand-stretch" | "sleepy-yawn",
  phase: CatMotionInterruptionPhase,
  renderedFrame: () => Uint8Array,
  frameSha256: (frame: Uint8Array) => Promise<string>,
): Promise<CatMotionInterruptionEvidenceV1> {
  assertRendererReady(renderer);
  const state = renderCatMotionEvidencePhase({
    playCatMotion: (name, transition) => renderer.playCatMotion(name, transition),
    setCatAutomationMode: (mode) => renderer.setCatAutomationMode?.(mode),
    setLookTarget: (target) => renderer.setLookTarget(target),
    update: (deltaMs) => renderer.update(deltaMs),
  }, {
    motion,
    timing: CAT_MOTION_EVIDENCE_TIMING[motion],
  }, { phase });
  const expectedState = phase === "interrupt-pet" ? "interrupted-pet" : "interrupted-drag";
  if (state !== expectedState) throw new Error(`motion interruption must enter ${expectedState}`);
  assertRendererReady(renderer);
  const frame = renderedFrame();
  if (renderedFramePixelCount(frame) <= 0) {
    throw new Error(`photo avatar interruption produced an empty framebuffer: ${phase}`);
  }
  renderer.setCatAutomationMode?.("paused");
  return {
    motion,
    phase,
    state: expectedState,
    framebufferNonEmpty: true,
    sha256: await frameSha256(frame),
    renderer: "cat-spatial-live2d-v1",
  };
}

export async function mountPhotoAvatarPreview(
  root: HTMLElement,
  sessionId: string,
  ports: PhotoAvatarPreviewPorts = browserPorts,
): Promise<PhotoAvatarPreviewHandle> {
  root.dataset.photoAvatarPreviewState = "loading";
  let asset: Awaited<ReturnType<typeof loadLive2DAsset>> | null = null;
  let renderer: PhotoAvatarLive2DRenderer | null = null;
  let resizeObserver: PreviewResizeObserver | null = null;
  let stopWatchingMotion: (() => void) | null = null;
  let frameId: number | undefined;
  let previousTimestamp: number | undefined;
  let idleMotion: PetMotionHandle | null = null;
  let evidence: CatMotionRuntimeEvidenceV1 | null = null;
  let destroyed = false;

  const cancelIdle = () => {
    if (frameId !== undefined) ports.cancelAnimationFrame(frameId);
    frameId = undefined;
    previousTimestamp = undefined;
    idleMotion?.cancel();
    idleMotion = null;
  };
  const destroy = () => {
    if (destroyed) return;
    destroyed = true;
    stopWatchingMotion?.();
    cancelIdle();
    resizeObserver?.disconnect();
    renderer?.destroy();
    asset?.dispose();
    root.replaceChildren();
    delete root.dataset.photoAvatarPreviewState;
    delete root.dataset.photoAvatarMotionEvidence;
  };

  try {
    const preview = await ports.previewManifest(sessionId);
    const manifest = requirePhotoAvatarManifest(preview.manifest);
    const transport = ports.previewTransport(sessionId, preview.revision, preview.manifest);
    asset = await ports.loadLive2DAsset(manifest.petId, manifest, transport);
    if (asset.motionSpatialProfile === undefined) {
      throw new Error("photo avatar preview requires a verified motion spatial profile");
    }

    const canvas = ports.createCanvas();
    canvas.className = "pet-render-surface";
    canvas.dataset.photoAvatarLive2dPreview = "1";
    root.replaceChildren(canvas);
    renderer = ports.createLive2DRenderer(canvas);
    const spatialRenderer = renderer;
    await spatialRenderer.load(asset);
    if (destroyed) throw new Error("photo avatar preview was destroyed while loading");
    assertRendererReady(spatialRenderer);
    assertSpatialRenderer(spatialRenderer);
    spatialRenderer.resize(viewport(root, ports.devicePixelRatio()));
    spatialRenderer.setVisibility(true);
    spatialRenderer.setCatAutomationMode?.("paused");
    spatialRenderer.update(0);
    if (ports.renderedPixelCount(canvas) <= 0) {
      throw new Error("photo avatar preview produced a blank WebGL frame");
    }

    if (preview.step === "runtimeCheckPending") {
      const frames: CatMotionFrameEvidenceV1[] = [];
      for (const motion of CAT_MOTION_SET_V1) {
        const motionFrames = await inspectMotion(
          spatialRenderer,
          motion,
          () => ports.renderedFrame(canvas),
          ports.frameSha256,
        );
        frames.push(...motionFrames);
      }
      const interruptions = [
        await inspectInterruption(
          spatialRenderer,
          "half-stand-stretch",
          "interrupt-pet",
          () => ports.renderedFrame(canvas),
          ports.frameSha256,
        ),
        await inspectInterruption(
          spatialRenderer,
          "sleepy-yawn",
          "interrupt-drag",
          () => ports.renderedFrame(canvas),
          ports.frameSha256,
        ),
      ];
      evidence = assertCompleteCatMotionEvidence({
        schemaVersion: 1,
        bodyModuleId: manifest.bodyModuleId,
        renderer: "cat-spatial-live2d-v1",
        frames,
        interruptions,
      });
      root.dataset.photoAvatarMotionEvidence = JSON.stringify(evidence);
      assertRendererReady(spatialRenderer);
      const manifestSha256 = await ports.manifestSha256(manifest);
      assertRendererReady(spatialRenderer);
      await ports.runtimeCheckPassed(sessionId, preview.revision, manifestSha256);
    }
    root.dataset.photoAvatarPreviewState = "previewReady";

    const resize = () => renderer?.resize(viewport(root, ports.devicePixelRatio()));
    resizeObserver = ports.createResizeObserver(resize);
    resizeObserver.observe(root);

    const render = (timestamp: number) => {
      if (destroyed) return;
      frameId = undefined;
      if (previousTimestamp !== undefined) renderer?.update(Math.max(0, timestamp - previousTimestamp));
      previousTimestamp = timestamp;
      frameId = ports.requestAnimationFrame(render);
    };
    const startIdle = () => {
      if (destroyed || idleMotion) return;
      idleMotion = spatialRenderer.playCatMotion("breathing", {
        loop: true,
        priority: 10,
        fadeInMs: 180,
        fadeOutMs: 140,
      });
      frameId = ports.requestAnimationFrame(render);
    };
    const applyMotionPreference = (reduced: boolean) => {
      if (reduced) {
        renderer?.setCatAutomationMode?.("paused");
        cancelIdle();
      } else {
        renderer?.setCatAutomationMode?.("idle");
        startIdle();
      }
    };
    stopWatchingMotion = ports.onReducedMotionChange(applyMotionPreference);
    applyMotionPreference(ports.prefersReducedMotion());

    return { destroy, evidence };
  } catch (error) {
    root.dataset.photoAvatarPreviewState = "error";
    root.dataset.photoAvatarPreviewError = error instanceof Error ? error.message : String(error);
    destroy();
    root.dataset.photoAvatarPreviewState = "error";
    throw error;
  }
}
