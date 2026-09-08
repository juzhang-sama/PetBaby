import { normalizeAssetPath } from "../runtime-assets/live2d-manifest";
import type { ManifestFileEntry } from "./manifest-schema";

export interface NormalizedRectV6 {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface FrameSequenceAction {
  actionId: string;
  loop: boolean;
  frameDurationMs: number;
  frames: string[];
  /**
   * 交互保持区间（帧下标闭区间）。播放器在一次性动作中进入该区间后不再前进，
   * 而是循环区间内帧，直到调用方以"释放"语义再次 playMotion 同一动作才继续播尾段。
   * 语义：拎起类互动（grab-release）的"悬空保持"段。
   */
  holdRange?: readonly [number, number];
}

export interface FrameSequenceBlink {
  enabled: boolean;
  minIntervalMs: number;
  maxIntervalMs: number;
}

export interface FrameSequenceIdleScheduleEntry {
  actionId: string;
  weight: number;
  minIntervalMs: number;
  maxIntervalMs: number;
}

export interface FrameSequenceIdleSchedule {
  entries: FrameSequenceIdleScheduleEntry[];
  minIntervalMs?: number; // fallback global interval; defaults to min of entries
  maxIntervalMs?: number; // fallback global interval; defaults to max of entries
  // Align one-shot trigger times to the default action's loop boundary.
  // Required when one-shot frames reuse the default action's phase-0 body
  // (e.g. composited blink frames) — triggering mid-loop would snap the body
  // back to phase 0 and cause a visible jump.
  alignToDefaultLoop?: boolean;
}

export interface RuntimeAssetManifestV6 {
  schemaVersion: 6;
  renderer: "frame-sequence-v1";
  petId: string;
  variantId: string;
  displayName: string;
  species: "cat" | "dog";
  baseImage: string;
  defaultAction: string;
  anchorPolicy: "fixed";
  actions: FrameSequenceAction[];
  semantics: Record<string, string>;
  blink?: FrameSequenceBlink;
  hitBounds?: NormalizedRectV6;
  files: ManifestFileEntry[];
}

export interface RuntimeAssetManifestV7 {
  schemaVersion: 7;
  renderer: "frame-sequence-v1";
  petId: string;
  variantId: string;
  displayName: string;
  species: "cat" | "dog";
  baseImage: string;
  defaultAction: string;
  anchorPolicy: "fixed";
  actions: FrameSequenceAction[];
  semantics: Record<string, string>;
  idleSchedule?: FrameSequenceIdleSchedule;
  blink?: FrameSequenceBlink; // deprecated: retained for schemaVersion 6 compatibility
  hitBounds?: NormalizedRectV6;
  files: ManifestFileEntry[];
}

const SHA256_HEX = /^[0-9a-f]{64}$/i;

function object(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${name} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requiredString(value: Record<string, unknown>, field: string): string {
  if (typeof value[field] !== "string" || value[field].length === 0) {
    throw new Error(`missing or invalid ${field}`);
  }
  return value[field] as string;
}

function requiredNumber(value: Record<string, unknown>, field: string): number {
  if (typeof value[field] !== "number" || !Number.isFinite(value[field])) {
    throw new Error(`missing or invalid ${field}`);
  }
  return value[field] as number;
}

function requiredRelativePngPath(value: Record<string, unknown>, field: string): string {
  let path: string;
  try {
    path = normalizeAssetPath(requiredString(value, field));
  } catch {
    throw new Error(`${field} must be a relative path`);
  }
  if (!isSupportedImagePath(path)) throw new Error(`${field} must be a PNG or WebP file`);
  return path;
}

/** 帧序列的位图帧支持 PNG（历史）与 WebP（体积瘦身）两种编码。 */
function isSupportedImagePath(path: string): boolean {
  const lower = path.toLowerCase();
  return lower.endsWith(".png") || lower.endsWith(".webp");
}

function parseRect(value: unknown, name: string): NormalizedRectV6 {
  const rect = object(value, name);
  const values = [rect.left, rect.top, rect.right, rect.bottom];
  for (const number of values) {
    if (typeof number !== "number" || !Number.isFinite(number)) {
      throw new Error(`${name} contains a non-finite value`);
    }
    if (number < 0 || number > 1) throw new Error(`${name} is out of range`);
  }
  const [left, top, right, bottom] = values as [number, number, number, number];
  if (left >= right || top >= bottom) throw new Error(`${name} is an inverted rect`);
  return { left, top, right, bottom };
}

function parseFrameSequenceAction(value: unknown, index: number): FrameSequenceAction {
  const action = object(value, `actions[${index}]`);
  const actionId = requiredString(action, "actionId");
  const loop = action.loop;
  if (typeof loop !== "boolean") throw new Error(`actions[${index}].loop must be a boolean`);
  const frameDurationMs = requiredNumber(action, "frameDurationMs");
  if (frameDurationMs <= 0) throw new Error(`actions[${index}].frameDurationMs must be positive`);
  const framesInput = action.frames;
  if (!Array.isArray(framesInput) || framesInput.length === 0) {
    throw new Error(`actions[${index}].frames must declare at least one frame`);
  }
  const seenFrames = new Set<string>();
  const frames = framesInput.map((frame, frameIndex) => {
    if (typeof frame !== "string" || frame.length === 0) {
      throw new Error(`actions[${index}].frames[${frameIndex}] must be a string`);
    }
    let path: string;
    try {
      path = normalizeAssetPath(frame);
    } catch {
      throw new Error(`actions[${index}].frames[${frameIndex}] must be a relative path`);
    }
    if (!isSupportedImagePath(path)) {
      throw new Error(`actions[${index}].frames[${frameIndex}] must be a PNG or WebP file`);
    }
    if (seenFrames.has(path)) {
      throw new Error(`duplicate frame path: ${path}`);
    }
    seenFrames.add(path);
    return path;
  });
  const result: FrameSequenceAction = { actionId, loop, frameDurationMs, frames };
  if (action.holdRange !== undefined) {
    const holdRange = action.holdRange;
    if (!Array.isArray(holdRange) || holdRange.length !== 2) {
      throw new Error(`actions[${index}].holdRange must be a [lo, hi] frame-index pair`);
    }
    const [lo, hi] = holdRange as unknown[];
    if (
      typeof lo !== "number" || typeof hi !== "number"
      || !Number.isInteger(lo) || !Number.isInteger(hi)
    ) {
      throw new Error(`actions[${index}].holdRange must contain integer frame indices`);
    }
    if (lo < 0 || hi >= frames.length || lo > hi) {
      throw new Error(
        `actions[${index}].holdRange must satisfy 0 <= lo <= hi < ${frames.length}`,
      );
    }
    result.holdRange = [lo, hi];
  }
  return result;
}

function parseFileEntries(value: unknown): ManifestFileEntry[] {
  if (!Array.isArray(value) || value.length === 0) throw new Error("manifest must declare files");
  const seenPaths = new Set<string>();
  return value.map((entry) => {
    const file = object(entry, "file entry");
    const role = requiredString(file, "role");
    const relativePath = normalizeAssetPath(requiredString(file, "relativePath"));
    const sha256 = requiredString(file, "sha256");
    if (!SHA256_HEX.test(sha256)) throw new Error("invalid file entry: sha256 must be 64 hex chars");
    if (seenPaths.has(relativePath)) throw new Error(`duplicate asset path: ${relativePath}`);
    seenPaths.add(relativePath);
    if (!isSupportedImagePath(relativePath) && !relativePath.toLowerCase().endsWith(".json")) {
      throw new Error(`unsupported asset extension: ${relativePath}`);
    }
    return { role, relativePath, sha256: sha256.toLowerCase() };
  });
}

function parseBlink(value: unknown): FrameSequenceBlink {
  const blink = object(value, "blink");
  const enabled = blink.enabled;
  if (typeof enabled !== "boolean") throw new Error("blink.enabled must be a boolean");
  const minIntervalMs = requiredNumber(blink, "minIntervalMs");
  const maxIntervalMs = requiredNumber(blink, "maxIntervalMs");
  if (minIntervalMs <= 0) throw new Error("blink.minIntervalMs must be positive");
  if (maxIntervalMs < minIntervalMs) {
    throw new Error("blink.maxIntervalMs must be >= minIntervalMs");
  }
  return { enabled, minIntervalMs, maxIntervalMs };
}

function parseIdleScheduleEntry(value: unknown, index: number): FrameSequenceIdleScheduleEntry {
  const entry = object(value, `idleSchedule.entries[${index}]`);
  const actionId = requiredString(entry, "actionId");
  const weight = requiredNumber(entry, "weight");
  const minIntervalMs = requiredNumber(entry, "minIntervalMs");
  const maxIntervalMs = entry.maxIntervalMs === undefined ? minIntervalMs : requiredNumber(entry, "maxIntervalMs");
  if (weight < 0) throw new Error(`idleSchedule.entries[${index}].weight must be >= 0`);
  if (minIntervalMs <= 0) throw new Error(`idleSchedule.entries[${index}].minIntervalMs must be positive`);
  if (maxIntervalMs < minIntervalMs) {
    throw new Error(`idleSchedule.entries[${index}].maxIntervalMs must be >= minIntervalMs`);
  }
  return { actionId, weight, minIntervalMs, maxIntervalMs };
}

function parseIdleSchedule(value: unknown): FrameSequenceIdleSchedule {
  const schedule = object(value, "idleSchedule");
  const entriesInput = schedule.entries;
  if (!Array.isArray(entriesInput) || entriesInput.length === 0) {
    throw new Error("idleSchedule.entries must be a non-empty array");
  }
  const seenActionIds = new Set<string>();
  const entries = entriesInput.map((entry, index) => {
    const parsed = parseIdleScheduleEntry(entry, index);
    if (seenActionIds.has(parsed.actionId)) {
      throw new Error(`idleSchedule.entries has duplicate actionId: ${parsed.actionId}`);
    }
    seenActionIds.add(parsed.actionId);
    return parsed;
  });
  const result: FrameSequenceIdleSchedule = { entries };
  if (schedule.alignToDefaultLoop !== undefined) {
    if (typeof schedule.alignToDefaultLoop !== "boolean") {
      throw new Error("idleSchedule.alignToDefaultLoop must be a boolean");
    }
    result.alignToDefaultLoop = schedule.alignToDefaultLoop;
  }
  if (schedule.minIntervalMs !== undefined) {
    result.minIntervalMs = requiredNumber(schedule, "minIntervalMs");
    if (result.minIntervalMs <= 0) throw new Error("idleSchedule.minIntervalMs must be positive");
  }
  if (schedule.maxIntervalMs !== undefined) {
    result.maxIntervalMs = requiredNumber(schedule, "maxIntervalMs");
    if (result.maxIntervalMs < (result.minIntervalMs ?? result.maxIntervalMs)) {
      throw new Error("idleSchedule.maxIntervalMs must be >= minIntervalMs");
    }
  }
  return result;
}

function blinkToIdleSchedule(blink: FrameSequenceBlink | undefined): FrameSequenceIdleSchedule | undefined {
  if (!blink || !blink.enabled) return undefined;
  return {
    entries: [
      {
        actionId: "blink",
        weight: 1,
        minIntervalMs: blink.minIntervalMs,
        maxIntervalMs: blink.maxIntervalMs,
      },
    ],
  };
}

export function parseFrameSequenceManifest(json: unknown): RuntimeAssetManifestV7 {
  const value = object(json, "manifest");
  const schemaVersion = value.schemaVersion;
  if (schemaVersion !== 6 && schemaVersion !== 7) {
    throw new Error(`unsupported schemaVersion: ${String(schemaVersion)}`);
  }
  if (value.renderer !== "frame-sequence-v1") {
    throw new Error(`unsupported renderer: ${String(value.renderer)}`);
  }
  const petId = requiredString(value, "petId");
  const variantId = requiredString(value, "variantId");
  const displayName = requiredString(value, "displayName");
  const species = value.species;
  if (species !== "cat" && species !== "dog") throw new Error("species must be cat or dog");
  const baseImage = requiredRelativePngPath(value, "baseImage");
  const defaultAction = requiredString(value, "defaultAction");
  if (value.anchorPolicy !== "fixed") throw new Error("anchorPolicy must be fixed");

  const actionsInput = value.actions;
  if (!Array.isArray(actionsInput) || actionsInput.length === 0) {
    throw new Error("manifest must declare at least one action");
  }
  const seenActionIds = new Set<string>();
  const actions = actionsInput.map((action, index) => {
    const parsed = parseFrameSequenceAction(action, index);
    if (seenActionIds.has(parsed.actionId)) {
      throw new Error(`duplicate actionId: ${parsed.actionId}`);
    }
    seenActionIds.add(parsed.actionId);
    return parsed;
  });
  if (!actions.some((action) => action.actionId === defaultAction)) {
    throw new Error("defaultAction must reference a declared action");
  }

  const semanticsValue = object(value.semantics, "semantics");
  const semantics: Record<string, string> = {};
  for (const [motion, actionId] of Object.entries(semanticsValue)) {
    if (typeof actionId !== "string" || actionId.length === 0) {
      throw new Error(`semantics.${motion} must be a non-empty string`);
    }
    if (!seenActionIds.has(actionId)) {
      throw new Error(`semantics.${motion} references unknown action: ${actionId}`);
    }
    semantics[motion] = actionId;
  }

  const blink = value.blink === undefined ? undefined : parseBlink(value.blink);
  const rawIdleSchedule = value.idleSchedule === undefined ? undefined : parseIdleSchedule(value.idleSchedule);
  const idleSchedule = rawIdleSchedule ?? blinkToIdleSchedule(blink);
  const hitBounds = value.hitBounds === undefined ? undefined : parseRect(value.hitBounds, "hitBounds");
  const files = parseFileEntries(value.files);

  return {
    schemaVersion: 7,
    renderer: "frame-sequence-v1",
    petId,
    variantId,
    displayName,
    species,
    baseImage,
    defaultAction,
    anchorPolicy: "fixed",
    actions,
    semantics,
    idleSchedule,
    blink,
    hitBounds,
    files,
  };
}
