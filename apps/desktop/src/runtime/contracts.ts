export const PROBE_VERSION = "m0" as const;
export const PREFERENCES_SCHEMA_VERSION = 2 as const;
export const MIN_DISPLAY_SCALE = 0.5;
export const MAX_DISPLAY_SCALE = 1.5;
export const PET_DISPLAY_SCALE_REQUEST = "pet-display-scale-request" as const;
export const PET_DISPLAY_SCALE_RESULT = "pet-display-scale-result" as const;
export const PET_CALIBRATION_PREVIEW_REQUEST = "pet-calibration-preview-request" as const;
export const PET_CALIBRATION_PREVIEW_RESULT = "pet-calibration-preview-result" as const;

export type WindowMode = "companion" | "desktop";
export type DesktopStrategy = "workerW" | "bottomFallback";
export type WindowModeSuppression =
  | "fullscreen"
  | "lockSleep"
  | "virtualDesktopMismatch"
  | "explorerLost"
  | "transition";

export interface WindowModeSnapshot {
  revision: number;
  desiredMode: WindowMode;
  actualMode: WindowMode | null;
  desktopStrategy: DesktopStrategy | null;
  userVisible: boolean;
  suppressions: WindowModeSuppression[];
}
export type RenderTier = "active" | "companion" | "still" | "paused";
export type PetCalibrationPreviewAction = "preview" | "restore" | "feedback" | "commit";

export interface PetCalibrationValue {
  schemaVersion: 1;
  breathAmplitudePercent: number;
  blinkIntervalScale: number;
  feedbackStrength: number;
}

export interface PetCalibrationPreviewRequest {
  requestId: string;
  petId: string;
  action: PetCalibrationPreviewAction;
  value: PetCalibrationValue;
}

export type PetCalibrationPreviewResult =
  | (PetCalibrationPreviewRequest & { ok: true })
  | {
    requestId: string;
    petId: string;
    action: PetCalibrationPreviewAction;
    ok: false;
    message: string;
  };

export interface RegionSpan {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface HitRegionPayload {
  canvasWidth: number;
  canvasHeight: number;
  scaleFactor: number;
  spans: RegionSpan[];
}

export interface ProbePreferences {
  schemaVersion: typeof PREFERENCES_SCHEMA_VERSION;
  x: number; y: number; width: number; height: number;
  displayScale: number; flipped: boolean; mode: WindowMode;
}

export interface WindowRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PetDisplayScaleRequest {
  requestId: string;
  displayScale: number;
}

export type PetDisplayScaleResult =
  | {
    requestId: string;
    requestedDisplayScale: number;
    ok: true;
    displayScale: number;
    rect: WindowRect;
  }
  | { requestId: string; requestedDisplayScale: number; ok: false; message: string };

const SAFE_REQUEST_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const SAFE_PET_ID = /^[A-Za-z0-9_-]{1,80}$/;

export function isSafeRequestId(value: unknown): value is string {
  return typeof value === "string" && SAFE_REQUEST_ID.test(value);
}

export function isSafePetId(value: unknown): value is string {
  return typeof value === "string" && SAFE_PET_ID.test(value);
}

function isDisplayScale(value: unknown): value is number {
  return typeof value === "number"
    && Number.isFinite(value)
    && value >= MIN_DISPLAY_SCALE
    && value <= MAX_DISPLAY_SCALE;
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

const WINDOW_MODES = new Set<unknown>(["companion", "desktop"]);
const DESKTOP_STRATEGIES = new Set<unknown>(["workerW", "bottomFallback"]);
const WINDOW_MODE_SUPPRESSIONS = new Set<unknown>([
  "fullscreen",
  "lockSleep",
  "virtualDesktopMismatch",
  "explorerLost",
  "transition",
]);

export function isWindowModeSnapshot(value: unknown): value is WindowModeSnapshot {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const snapshot = value as Record<string, unknown>;
  if (!hasExactKeys(snapshot, [
    "revision",
    "desiredMode",
    "actualMode",
    "desktopStrategy",
    "userVisible",
    "suppressions",
  ])) return false;
  if (!Number.isSafeInteger(snapshot.revision) || (snapshot.revision as number) < 0
    || !WINDOW_MODES.has(snapshot.desiredMode)
    || !(snapshot.actualMode === null || WINDOW_MODES.has(snapshot.actualMode))
    || !(snapshot.desktopStrategy === null || DESKTOP_STRATEGIES.has(snapshot.desktopStrategy))
    || typeof snapshot.userVisible !== "boolean"
    || !Array.isArray(snapshot.suppressions)
    || !snapshot.suppressions.every((reason) => WINDOW_MODE_SUPPRESSIONS.has(reason))) {
    return false;
  }
  return snapshot.actualMode === "desktop"
    ? snapshot.desktopStrategy !== null
    : snapshot.desktopStrategy === null;
}

function isCalibrationValue(value: unknown): value is PetCalibrationValue {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const calibration = value as Record<string, unknown>;
  return hasExactKeys(calibration, [
    "schemaVersion",
    "breathAmplitudePercent",
    "blinkIntervalScale",
    "feedbackStrength",
  ])
    && calibration.schemaVersion === 1
    && isFiniteNumberInRange(calibration.breathAmplitudePercent, 0, 5)
    && isFiniteNumberInRange(calibration.blinkIntervalScale, 0.5, 2)
    && isFiniteNumberInRange(calibration.feedbackStrength, 0, 1);
}

function isFiniteNumberInRange(value: unknown, min: number, max: number): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= min && value <= max;
}

function isCalibrationAction(value: unknown): value is PetCalibrationPreviewAction {
  return value === "preview" || value === "restore" || value === "feedback" || value === "commit";
}

export function isPetCalibrationPreviewRequest(value: unknown): value is PetCalibrationPreviewRequest {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const request = value as Record<string, unknown>;
  return hasExactKeys(request, ["requestId", "petId", "action", "value"])
    && isSafeRequestId(request.requestId)
    && isSafePetId(request.petId)
    && isCalibrationAction(request.action)
    && isCalibrationValue(request.value);
}

export function isPetCalibrationPreviewResult(value: unknown): value is PetCalibrationPreviewResult {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const result = value as Record<string, unknown>;
  if (!isSafeRequestId(result.requestId)
    || !isSafePetId(result.petId)
    || !isCalibrationAction(result.action)) return false;
  if (result.ok === true) {
    return hasExactKeys(result, ["requestId", "petId", "action", "value", "ok"])
      && isCalibrationValue(result.value);
  }
  return result.ok === false
    && hasExactKeys(result, ["requestId", "petId", "action", "ok", "message"])
    && typeof result.message === "string"
    && result.message.length > 0
    && result.message.length <= 2_048;
}

function isActualWindowRect(value: unknown): value is WindowRect {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const rect = value as Record<string, unknown>;
  return hasExactKeys(rect, ["x", "y", "width", "height"])
    && [rect.x, rect.y, rect.width, rect.height].every(
      (entry) => typeof entry === "number" && Number.isFinite(entry),
    )
    && (rect.width as number) > 0
    && (rect.height as number) > 0;
}

export function isPetDisplayScaleRequest(value: unknown): value is PetDisplayScaleRequest {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const request = value as Record<string, unknown>;
  return hasExactKeys(request, ["requestId", "displayScale"])
    && isSafeRequestId(request.requestId)
    && isDisplayScale(request.displayScale);
}

export function isPetDisplayScaleResult(value: unknown): value is PetDisplayScaleResult {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const result = value as Record<string, unknown>;
  if (!isSafeRequestId(result.requestId)) return false;
  if (result.ok === true) {
    return hasExactKeys(result, [
      "requestId",
      "requestedDisplayScale",
      "ok",
      "displayScale",
      "rect",
    ])
      && isDisplayScale(result.requestedDisplayScale)
      && isDisplayScale(result.displayScale)
      && isActualWindowRect(result.rect);
  }
  return result.ok === false
    && hasExactKeys(result, ["requestId", "requestedDisplayScale", "ok", "message"])
    && isDisplayScale(result.requestedDisplayScale)
    && typeof result.message === "string"
    && result.message.length > 0
    && result.message.length <= 2_048;
}

const isIntegerInRange = (value: unknown, min: number, max: number): value is number => (
  typeof value === "number"
  && Number.isInteger(value)
  && value >= min
  && value <= max
);

export function isProbePreferences(value: unknown): value is ProbePreferences {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const preferences = value as Record<string, unknown>;
  return preferences.schemaVersion === PREFERENCES_SCHEMA_VERSION
    && isIntegerInRange(preferences.x, -0x8000_0000, 0x7fff_ffff)
    && isIntegerInRange(preferences.y, -0x8000_0000, 0x7fff_ffff)
    && isIntegerInRange(preferences.width, 1, 0xffff_ffff)
    && isIntegerInRange(preferences.height, 1, 0xffff_ffff)
    && typeof preferences.displayScale === "number"
    && Number.isFinite(preferences.displayScale)
    && preferences.displayScale >= MIN_DISPLAY_SCALE
    && preferences.displayScale <= MAX_DISPLAY_SCALE
    && typeof preferences.flipped === "boolean"
    && (preferences.mode === "companion" || preferences.mode === "desktop");
}
