import { parseCatCharacterManifest } from "../runtime-assets/cat-character-manifest";
import { loadLive2DAsset } from "../runtime-assets/live2d-asset-loader";
import { createPetRendererRuntime } from "../runtime/pet-renderer-bootstrap";
import {
  createBuiltinPetTransport,
  resolveBuiltinPetUrl,
} from "../runtime/startup-pet";
import {
  CAT_MOTION_SET_V1,
  type CatMotionNameV1,
} from "./cat-motion-contract";
import {
  BODY_MODULE_IDS_V1,
  type BodyModuleIdV1,
} from "../runtime-assets/cat-motion-spatial-profile";
import {
  CAT_BLINK_DURATION_MS,
  CAT_FIRST_BLINK_DELAY_MS,
  type CatAutomationMode,
} from "./cat-automation";

const STANDARD_CAT_ID = "cat-a-standard-v1";
const STANDARD_CAT_MANIFEST_URL = `/builtin-pets/${STANDARD_CAT_ID}/manifest.json`;

export interface CatMotionEvidenceTiming {
  durationMs: number;
  peakMs: number;
  fallbackMs: number;
}

export const CAT_MOTION_EVIDENCE_TIMING: Readonly<Record<CatMotionNameV1, CatMotionEvidenceTiming>> = {
  breathing: { durationMs: 4_000, peakMs: 2_000, fallbackMs: 4_450 },
  blink: {
    durationMs: CAT_BLINK_DURATION_MS,
    peakMs: CAT_BLINK_DURATION_MS / 2,
    fallbackMs: CAT_BLINK_DURATION_MS + 400,
  },
  "ear-twitch": { durationMs: 800, peakMs: 420, fallbackMs: 1_200 },
  "tail-idle": { durationMs: 3_200, peakMs: 1_600, fallbackMs: 3_650 },
  "pointer-focus": { durationMs: 1_200, peakMs: 600, fallbackMs: 1_650 },
  "pet-happy": { durationMs: 1_800, peakMs: 900, fallbackMs: 2_250 },
  "sleepy-yawn": { durationMs: 2_600, peakMs: 1_400, fallbackMs: 3_050 },
  "half-stand-stretch": { durationMs: 2_400, peakMs: 1_300, fallbackMs: 2_850 },
};

export type CatMotionEvidencePhase = "neutral" | "peak" | "fallback" | "frame" | "interrupt-pet" | "interrupt-drag";
export type ParsedCatMotionEvidencePhase =
  | { phase: Exclude<CatMotionEvidencePhase, "frame"> }
  | { phase: "frame"; atMs: number };

export const CAT_MOTION_EVIDENCE_FRAME_PHASES = ["neutral", "peak", "fallback"] as const;
export type CatMotionEvidenceFramePhase = (typeof CAT_MOTION_EVIDENCE_FRAME_PHASES)[number];
export type CatMotionInterruptionPhase = "interrupt-pet" | "interrupt-drag";
export type CatMotionInterruptionState = "interrupted-pet" | "interrupted-drag";

export interface CatMotionFrameEvidenceV1 {
  motion: CatMotionNameV1;
  phase: CatMotionEvidenceFramePhase;
  framebufferNonEmpty: true;
  changedPixelCount: number;
  sha256: string;
  renderer: "cat-spatial-live2d-v1";
}

export interface CatMotionInterruptionEvidenceV1 {
  motion: "half-stand-stretch" | "sleepy-yawn";
  phase: CatMotionInterruptionPhase;
  state: CatMotionInterruptionState;
  framebufferNonEmpty: true;
  sha256: string;
  renderer: "cat-spatial-live2d-v1";
}

export interface CatMotionRuntimeEvidenceV1 {
  schemaVersion: 1;
  bodyModuleId: BodyModuleIdV1;
  renderer: "cat-spatial-live2d-v1";
  frames: CatMotionFrameEvidenceV1[];
  interruptions: CatMotionInterruptionEvidenceV1[];
}

const SHA256_HEX = /^[a-f0-9]{64}$/;

export function assertCompleteCatMotionEvidence(
  evidence: CatMotionRuntimeEvidenceV1,
): CatMotionRuntimeEvidenceV1 {
  if (!(BODY_MODULE_IDS_V1 as readonly string[]).includes(evidence.bodyModuleId)) {
    throw new Error(`unsupported body module evidence: ${evidence.bodyModuleId}`);
  }
  if (evidence.schemaVersion !== 1 || evidence.renderer !== "cat-spatial-live2d-v1") {
    throw new Error("motion evidence requires the spatial Live2D renderer");
  }

  const frames = new Map<string, CatMotionFrameEvidenceV1>();
  for (const frame of evidence.frames) {
    const key = `${frame.motion}:${frame.phase}`;
    if (frames.has(key)) throw new Error(`duplicate motion evidence frame: ${key}`);
    if (!frame.framebufferNonEmpty || !SHA256_HEX.test(frame.sha256)) {
      throw new Error(`motion evidence framebuffer/hash is invalid: ${key}`);
    }
    if (frame.renderer !== evidence.renderer) {
      throw new Error(`motion evidence renderer mismatch: ${key}`);
    }
    if (frame.phase !== "neutral" && frame.changedPixelCount <= 20) {
      throw new Error(`motion evidence lacks visible pixel change: ${key}`);
    }
    frames.set(key, frame);
  }
  for (const motion of CAT_MOTION_SET_V1) {
    for (const phase of CAT_MOTION_EVIDENCE_FRAME_PHASES) {
      if (!frames.has(`${motion}:${phase}`)) {
        throw new Error(`missing motion evidence frame: ${motion}:${phase}`);
      }
    }
  }
  if (frames.size !== CAT_MOTION_SET_V1.length * CAT_MOTION_EVIDENCE_FRAME_PHASES.length) {
    throw new Error("motion evidence contains an unsupported frame");
  }

  const stretchHashes = new Set(CAT_MOTION_EVIDENCE_FRAME_PHASES.map((phase) => (
    frames.get(`half-stand-stretch:${phase}`)!.sha256
  )));
  if (stretchHashes.size !== CAT_MOTION_EVIDENCE_FRAME_PHASES.length) {
    throw new Error("half-stand-stretch neutral, peak, and fallback must be distinct");
  }

  const requiredInterruptions = new Map<CatMotionInterruptionPhase, {
    motion: CatMotionInterruptionEvidenceV1["motion"];
    state: CatMotionInterruptionState;
  }>([
    ["interrupt-pet", { motion: "half-stand-stretch", state: "interrupted-pet" }],
    ["interrupt-drag", { motion: "sleepy-yawn", state: "interrupted-drag" }],
  ]);
  if (evidence.interruptions.length !== requiredInterruptions.size) {
    throw new Error("motion evidence requires both interruption states");
  }
  for (const interruption of evidence.interruptions) {
    const expected = requiredInterruptions.get(interruption.phase);
    if (
      expected === undefined
      || interruption.motion !== expected.motion
      || interruption.state !== expected.state
    ) {
      throw new Error(`motion interruption must enter ${expected?.state ?? "a supported interrupted state"}`);
    }
    if (!interruption.framebufferNonEmpty || !SHA256_HEX.test(interruption.sha256)) {
      throw new Error(`motion interruption framebuffer/hash is invalid: ${interruption.phase}`);
    }
    if (interruption.renderer !== evidence.renderer) {
      throw new Error(`motion interruption renderer mismatch: ${interruption.phase}`);
    }
    requiredInterruptions.delete(interruption.phase);
  }
  if (requiredInterruptions.size > 0) throw new Error("motion evidence is missing an interruption state");
  return evidence;
}

export interface CatMotionEvidenceRequest {
  motion: CatMotionNameV1;
  timing: CatMotionEvidenceTiming;
}

export interface CatMotionEvidenceSession {
  destroy(): void;
}

interface CatMotionEvidencePlaybackHost {
  playCatMotion(
    motion: CatMotionNameV1,
    transition: {
      loop?: boolean;
      priority?: number;
      fadeInMs?: number;
      fadeOutMs?: number;
    },
  ): unknown;
  setCatAutomationMode(mode: CatAutomationMode): void;
  setLookTarget(target: { x: number; y: number } | null): void;
  update(deltaMs: number): void;
}

function isCatMotionName(value: string | null): value is CatMotionNameV1 {
  return value !== null && (CAT_MOTION_SET_V1 as readonly string[]).includes(value);
}

export function parseCatMotionEvidencePhase(
  search: string,
  durationMs: number,
): ParsedCatMotionEvidencePhase {
  const query = new URLSearchParams(search);
  const value = query.get("phase");
  if (value === "frame") {
    const requested = Number(query.get("atMs"));
    const maximum = Math.max(0, durationMs - 1);
    return {
      phase: "frame",
      atMs: Math.min(maximum, Math.max(0, Number.isFinite(requested) ? requested : 0)),
    };
  }
  if (
    value === "neutral"
    || value === "peak"
    || value === "fallback"
    || value === "interrupt-pet"
    || value === "interrupt-drag"
  ) {
    return { phase: value };
  }
  return { phase: "peak" };
}

export function isCatMotionEvidenceMode(search: string): boolean {
  const query = new URLSearchParams(search);
  return query.get("catMotionEvidence") === "1" && isCatMotionName(query.get("motion"));
}

export function parseCatMotionEvidenceRequest(search: string): CatMotionEvidenceRequest {
  const motion = new URLSearchParams(search).get("motion");
  if (!isCatMotionName(motion)) throw new Error(`unsupported evidence motion: ${String(motion)}`);
  return { motion, timing: CAT_MOTION_EVIDENCE_TIMING[motion] };
}

export function advanceCatMotionEvidenceTime(
  update: (deltaMs: number) => void,
  totalMs: number,
  stepMs = 1_000 / 60,
): void {
  const durationMs = Math.max(0, totalMs);
  const fixedStepMs = Number.isFinite(stepMs) && stepMs > 0 ? stepMs : 1_000 / 60;
  if (durationMs === 0) {
    update(0);
    return;
  }
  let elapsedMs = 0;
  while (elapsedMs < durationMs) {
    const deltaMs = Math.min(fixedStepMs, durationMs - elapsedMs);
    update(deltaMs);
    elapsedMs += deltaMs;
  }
}

export function renderCatMotionEvidencePhase(
  host: CatMotionEvidencePlaybackHost,
  request: CatMotionEvidenceRequest,
  phaseRequest: ParsedCatMotionEvidencePhase,
): string {
  const phase = phaseRequest.phase;
  if (phase === "neutral") {
    host.update(0);
    return "ready";
  }

  if (request.motion === "blink" && (
    phase === "peak"
    || phase === "frame"
    || phase === "fallback"
  )) {
    host.setCatAutomationMode("idle");
    const phaseMs = phase === "peak"
      ? request.timing.peakMs
      : phase === "frame"
        ? phaseRequest.atMs
        : request.timing.fallbackMs;
    advanceCatMotionEvidenceTime(
      (deltaMs) => host.update(deltaMs),
      CAT_FIRST_BLINK_DELAY_MS + phaseMs,
    );
    return phase;
  }

  host.playCatMotion(request.motion, {
    priority: 60,
    fadeInMs: 160,
    fadeOutMs: 220,
  });
  if (request.motion === "breathing") host.setCatAutomationMode("idle");

  if (request.motion === "pointer-focus" && (phase === "peak" || phase === "frame")) {
    host.setLookTarget({ x: 0.65, y: 0.35 });
  } else {
    host.setLookTarget(null);
  }

  if (phase === "peak") {
    advanceCatMotionEvidenceTime((deltaMs) => host.update(deltaMs), request.timing.peakMs);
    return "peak";
  }
  if (phase === "frame") {
    advanceCatMotionEvidenceTime((deltaMs) => host.update(deltaMs), phaseRequest.atMs);
    return "frame";
  }
  if (phase === "fallback") {
    advanceCatMotionEvidenceTime((deltaMs) => host.update(deltaMs), request.timing.fallbackMs);
    host.playCatMotion("breathing", {
      priority: 10,
      loop: true,
      fadeInMs: 180,
      fadeOutMs: 140,
    });
    advanceCatMotionEvidenceTime((deltaMs) => host.update(deltaMs), 240);
    return "fallback";
  }

  const interruptDelay = Math.min(request.timing.peakMs, 900);
  advanceCatMotionEvidenceTime((deltaMs) => host.update(deltaMs), interruptDelay);
  if (phase === "interrupt-pet") {
    host.playCatMotion("pet-happy", {
      priority: 90,
      fadeInMs: 120,
      fadeOutMs: 180,
    });
    advanceCatMotionEvidenceTime((deltaMs) => host.update(deltaMs), 260);
    return "interrupted-pet";
  }

  host.playCatMotion("breathing", {
    priority: 100,
    loop: true,
    fadeInMs: 180,
    fadeOutMs: 140,
  });
  host.setCatAutomationMode("dragging");
  advanceCatMotionEvidenceTime((deltaMs) => host.update(deltaMs), 260);
  return "interrupted-drag";
}

async function sampleEvidenceFps(
  update: (deltaMs: number) => void,
): Promise<number> {
  const frameDeltas: number[] = [];
  let previousTime = performance.now();
  await new Promise<void>((resolve) => {
    const render = (time: number): void => {
      const deltaMs = Math.min(100, Math.max(0, time - previousTime));
      update(deltaMs);
      if (deltaMs > 0) frameDeltas.push(deltaMs);
      previousTime = time;
      if (frameDeltas.length >= 30) {
        resolve();
        return;
      }
      window.requestAnimationFrame(render);
    };
    window.requestAnimationFrame(render);
  });
  const mean = frameDeltas.reduce((sum, value) => sum + value, 0) / frameDeltas.length;
  return 1_000 / mean;
}

export async function mountCatMotionEvidence(
  root: HTMLElement,
  search = window.location.search,
): Promise<CatMotionEvidenceSession> {
  const request = parseCatMotionEvidenceRequest(search);
  const phaseRequest = parseCatMotionEvidencePhase(search, request.timing.durationMs);
  const phase = phaseRequest.phase;
  const manifestUrl = STANDARD_CAT_MANIFEST_URL;
  const transport = createBuiltinPetTransport({ manifestUrl });
  const manifest = parseCatCharacterManifest(await transport.readManifest(STANDARD_CAT_ID));

  root.dataset.catMotionEvidence = "loading";
  root.dataset.evidenceMotion = request.motion;
  root.dataset.evidencePhase = phase;
  root.style.background = "repeating-conic-gradient(#eef2f6 0 25%, #d9e0e8 0 50%) 50% / 32px 32px";

  const runtime = await createPetRendererRuntime(STANDARD_CAT_ID, manifest, {
    root,
    assetUrl: (_petId, relativePath) => resolveBuiltinPetUrl(
      manifestUrl,
      relativePath,
      window.location.origin,
    ),
    loadLive2DAsset: (petId, expected) => loadLive2DAsset(petId, expected, transport),
  });
  const host = runtime.host;
  host.resize({ width: 420, height: 520, dpr: Math.max(1, window.devicePixelRatio || 1) });
  host.setVisibility(true);
  host.setCatAutomationMode("paused");

  host.update(0);
  root.dataset.evidenceFps = (await sampleEvidenceFps((deltaMs) => host.update(deltaMs))).toFixed(2);
  root.dataset.catMotionEvidence = renderCatMotionEvidencePhase(host, request, phaseRequest);
  if (phase === "frame") root.dataset.evidenceAtMs = String(phaseRequest.atMs);
  root.dataset.evidenceFrozen = "1";

  return {
    destroy: () => {
      host.destroy();
      delete root.dataset.catMotionEvidence;
      delete root.dataset.evidenceFrozen;
    },
  };
}
