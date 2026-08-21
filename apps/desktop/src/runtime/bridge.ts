import { invoke } from "@tauri-apps/api/core";
import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { emitTo, listen } from "@tauri-apps/api/event";
import {
  isPetDisplayScaleRequest,
  isPetDisplayScaleResult,
  isSafeRequestId,
  MAX_DISPLAY_SCALE,
  MIN_DISPLAY_SCALE,
  PET_DISPLAY_SCALE_REQUEST,
  PET_DISPLAY_SCALE_RESULT,
  type HitRegionPayload,
  type PetDisplayScaleRequest,
  type PetDisplayScaleResult,
  type ProbePreferences,
} from "./contracts";
import { isFullscreenSnapshot, type FullscreenSnapshot } from "./fullscreen";
import type {
  WindowRect,
  WindowSizeAck,
  WindowSizeApplyError,
  WindowSizePort,
} from "./window-size-controller";

export interface HitRegionEvidence {
  spanCount: number;
  applied: boolean;
  strategy: string;
  scaleFactor: number;
}

export function applyHitRegion(payload: HitRegionPayload): Promise<HitRegionEvidence> {
  return invoke("apply_hit_region", { payload });
}

export const loadPreferences = (): Promise<ProbePreferences> => invoke("load_preferences");
export const savePreferences = (value: ProbePreferences): Promise<void> => invoke("save_preferences", { value });
export const beginDrag = (): Promise<void> => invoke("begin_drag");

export const probeFullscreen = async (): Promise<FullscreenSnapshot> => {
  const value = await invoke<unknown>("probe_fullscreen");
  if (!isFullscreenSnapshot(value)) throw new TypeError("Invalid fullscreen snapshot");
  return value;
};
export const updateWindowFullscreen = (snapshot: FullscreenSnapshot): Promise<unknown> =>
  invoke("window_fullscreen_update", { active: snapshot.isFullscreen });
export const reconcileWindowVisibility = (): Promise<unknown> =>
  invoke("window_visibility_reconcile");

export interface AssetHealth {
  petId: string;
  status: "healthy" | "corrupt" | "missing";
  manifestPath: string;
}

export const assetScan = (): Promise<AssetHealth[]> => invoke("asset_scan");

export interface DisplayScaleClientPorts {
  listen(handler: (result: unknown) => void): Promise<() => void>;
  emit(request: PetDisplayScaleRequest): Promise<void>;
}

export interface RequestDisplayScaleOptions {
  ports?: DisplayScaleClientPorts;
  requestIdFactory?: () => string;
  timeoutMs?: number;
}

const tauriDisplayScaleClientPorts: DisplayScaleClientPorts = {
  listen: async (handler) => listen<unknown>(PET_DISPLAY_SCALE_RESULT, ({ payload }) => handler(payload)),
  emit: (request) => emitTo("pet", PET_DISPLAY_SCALE_REQUEST, request),
};

const activeDisplayScaleRequestIds = new Set<string>();

export function requestPetDisplayScale(
  displayScale: number,
  options: RequestDisplayScaleOptions = {},
): Promise<PetDisplayScaleResult> {
  if (!Number.isFinite(displayScale)
    || displayScale < MIN_DISPLAY_SCALE
    || displayScale > MAX_DISPLAY_SCALE) {
    return Promise.reject(new RangeError(
      `displayScale must be between ${MIN_DISPLAY_SCALE} and ${MAX_DISPLAY_SCALE}`,
    ));
  }
  const requestId = (options.requestIdFactory ?? (() => crypto.randomUUID()))();
  if (!isSafeRequestId(requestId)) {
    return Promise.reject(new TypeError("requestId factory returned an unsafe identifier"));
  }
  const ports = options.ports ?? tauriDisplayScaleClientPorts;
  const timeoutMs = options.timeoutMs ?? 5_000;
  if (!Number.isFinite(timeoutMs) || timeoutMs < 0) {
    return Promise.reject(new RangeError("timeoutMs must be a finite non-negative number"));
  }
  if (activeDisplayScaleRequestIds.has(requestId)) {
    return Promise.reject(new Error(`Display scale request id is already active: ${requestId}`));
  }
  activeDisplayScaleRequestIds.add(requestId);

  let resolveResult!: (result: PetDisplayScaleResult) => void;
  let rejectResult!: (error: Error) => void;
  const resultPromise = new Promise<PetDisplayScaleResult>((resolve, reject) => {
    resolveResult = resolve;
    rejectResult = reject;
  });
  let settled = false;
  let unlisten: (() => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cleanup = (): void => {
    activeDisplayScaleRequestIds.delete(requestId);
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
    const dispose = unlisten;
    unlisten = undefined;
    try {
      dispose?.();
    } catch {
      // Cleanup must never change the request result.
    }
  };
  const rejectOnce = (error: unknown): void => {
    if (settled) return;
    settled = true;
    cleanup();
    rejectResult(error instanceof Error ? error : new Error(String(error)));
  };
  const resolveOnce = (result: PetDisplayScaleResult): void => {
    if (settled) return;
    settled = true;
    cleanup();
    resolveResult(result);
  };

  timer = setTimeout(() => {
    rejectOnce(new Error(`Pet display scale request timed out after ${timeoutMs / 1_000} seconds`));
  }, timeoutMs);
  void (async () => {
    try {
      const dispose = await ports.listen((candidate) => {
        if (!isPetDisplayScaleResult(candidate)
          || candidate.requestId !== requestId
          || candidate.requestedDisplayScale !== displayScale) return;
        resolveOnce(candidate);
      });
      if (settled) {
        try {
          dispose();
        } catch {
          // A late listener cannot reopen a settled request.
        }
        return;
      }
      unlisten = dispose;
      await ports.emit({ requestId, displayScale });
    } catch (error) {
      rejectOnce(error);
    }
  })();
  return resultPromise;
}

interface PhysicalPointLike {
  toLogical(scaleFactor: number): { x: number; y: number };
}

interface PhysicalSizeLike {
  toLogical(scaleFactor: number): { width: number; height: number };
}

export interface PhysicalWindowGeometry {
  position?: PhysicalPointLike;
  size?: PhysicalSizeLike;
}

export interface LogicalWindowBoundary {
  scaleFactor(): Promise<number>;
  outerPosition(): Promise<PhysicalPointLike>;
  outerSize(): Promise<PhysicalSizeLike>;
  setPosition(position: LogicalPosition): Promise<void>;
  setSize(size: LogicalSize): Promise<void>;
}

export interface LogicalMonitorBoundary {
  scaleFactor: number;
  workArea: { position: PhysicalPointLike; size: PhysicalSizeLike };
}

export interface LogicalWindowSizePortOptions {
  window: LogicalWindowBoundary;
  currentMonitor(): Promise<LogicalMonitorBoundary | null>;
  resizeRenderer(): Promise<void>;
  refreshHitRegion(): Promise<void>;
}

type LogicalWindowReader = Pick<
  LogicalWindowBoundary,
  "scaleFactor" | "outerPosition" | "outerSize"
>;

export async function readLogicalWindowRect(
  window: LogicalWindowReader,
  geometry: PhysicalWindowGeometry = {},
): Promise<WindowRect> {
  const [scaleFactor, position, size] = await Promise.all([
    window.scaleFactor(),
    geometry.position ?? window.outerPosition(),
    geometry.size ?? window.outerSize(),
  ]);
  if (!Number.isFinite(scaleFactor) || scaleFactor <= 0) {
    throw new RangeError("Window scale factor must be finite and positive");
  }
  const logicalPosition = position.toLogical(scaleFactor);
  const logicalSize = size.toLogical(scaleFactor);
  const rect = {
    x: logicalPosition.x,
    y: logicalPosition.y,
    width: logicalSize.width,
    height: logicalSize.height,
  };
  if (![rect.x, rect.y, rect.width, rect.height].every(Number.isFinite)
    || rect.width <= 0
    || rect.height <= 0) {
    throw new RangeError("Logical window rectangle must be finite with a positive size");
  }
  return rect;
}

export function createLogicalWindowSizePort(options: LogicalWindowSizePortOptions): WindowSizePort {
  return {
    getRect: async () => {
      return readLogicalWindowRect(options.window);
    },
    getWorkArea: async () => {
      const monitor = await options.currentMonitor();
      if (!monitor) throw new Error("Cannot resize pet window without a current monitor");
      const position = monitor.workArea.position.toLogical(monitor.scaleFactor);
      const size = monitor.workArea.size.toLogical(monitor.scaleFactor);
      return { x: position.x, y: position.y, width: size.width, height: size.height };
    },
    setRect: async (rect) => {
      await options.window.setSize(new LogicalSize(rect.width, rect.height));
      await options.window.setPosition(new LogicalPosition(rect.x, rect.y));
    },
    resizeRenderer: options.resizeRenderer,
    refreshHitRegion: options.refreshHitRegion,
  };
}

export interface LogicalWindowGeometryPersistenceOptions {
  window: LogicalWindowReader;
  preferences: ProbePreferences;
  save?: (value: ProbePreferences) => Promise<void>;
  diagnose?(stage: "window-geometry", error: unknown): void;
}

export interface LogicalWindowGeometryPersistence {
  persist(geometry?: PhysicalWindowGeometry): Promise<void>;
  flushCurrentGeometry(): Promise<void>;
  commitDisplayScale(ack: WindowSizeAck): Promise<void>;
  runDisplayScaleTransaction<T>(operation: () => Promise<T>): Promise<T>;
}

export function createLogicalWindowGeometryPersistence(
  options: LogicalWindowGeometryPersistenceOptions,
): LogicalWindowGeometryPersistence {
  let latestGeneration = 0;
  let saveQueue: Promise<void> = Promise.resolve();
  let lastPersistedSnapshot: ProbePreferences = { ...options.preferences };
  let scaleTransactionActive = false;
  let scaleTransactionBaseline: ProbePreferences | null = null;
  let scaleTransactionCommitted = false;
  const save = options.save ?? savePreferences;
  const diagnose = (error: unknown): void => {
    try {
      options.diagnose?.("window-geometry", error);
    } catch {
      // Diagnostics cannot make native window event callbacks reject.
    }
  };
  const enqueue = (action: () => Promise<void>): Promise<void> => {
    const transaction = saveQueue.then(action);
    saveQueue = transaction.catch(() => undefined);
    return transaction;
  };
  const publishPersisted = (value: ProbePreferences): void => {
    lastPersistedSnapshot = { ...value };
    Object.assign(options.preferences, lastPersistedSnapshot);
  };
  const persistGeometry = async (
    geometry: PhysicalWindowGeometry,
    strict: boolean,
  ): Promise<void> => {
    if (scaleTransactionActive) return;
    const generation = ++latestGeneration;
    let rect: WindowRect;
    try {
      rect = await readLogicalWindowRect(options.window, geometry);
    } catch (error) {
      diagnose(error);
      if (strict) throw error;
      return;
    }
    if (!strict && generation !== latestGeneration) return;

    const transaction = enqueue(async () => {
      if (!strict && generation !== latestGeneration) return;
      const next: ProbePreferences = {
        ...lastPersistedSnapshot,
        x: Math.round(rect.x),
        y: Math.round(rect.y),
        width: Math.max(1, Math.round(rect.width)),
        height: Math.max(1, Math.round(rect.height)),
      };
      try {
        await save({ ...next });
        publishPersisted(next);
      } catch (error) {
        diagnose(error);
        if (strict) throw error;
      }
    });
    await transaction;
  };

  return {
    persist: (geometry = {}) => persistGeometry(geometry, false),
    flushCurrentGeometry: () => persistGeometry({}, true),
    commitDisplayScale: (ack) => {
      if (!scaleTransactionActive || !scaleTransactionBaseline) {
        return Promise.reject(new Error("Display scale commit requires an active transaction"));
      }
      if (scaleTransactionCommitted) {
        return Promise.reject(new Error("Display scale transaction was already committed"));
      }
      scaleTransactionCommitted = true;
      return (async () => {
        const next: ProbePreferences = {
          ...scaleTransactionBaseline,
          displayScale: ack.appliedScale,
          x: Math.round(ack.rect.x),
          y: Math.round(ack.rect.y),
          width: Math.max(1, Math.round(ack.rect.width)),
          height: Math.max(1, Math.round(ack.rect.height)),
        };
        Object.assign(options.preferences, next);
        try {
          await save({ ...next });
          publishPersisted(next);
        } catch (error) {
          Object.assign(options.preferences, scaleTransactionBaseline);
          throw error;
        }
      })();
    },
    runDisplayScaleTransaction: async (operation) => {
      if (scaleTransactionActive) {
        throw new Error("A display scale transaction is already active");
      }
      scaleTransactionActive = true;
      scaleTransactionCommitted = false;
      latestGeneration += 1;
      try {
        await saveQueue;
        scaleTransactionBaseline = { ...lastPersistedSnapshot };
        return await operation();
      } finally {
        scaleTransactionBaseline = null;
        scaleTransactionCommitted = false;
        scaleTransactionActive = false;
      }
    },
  };
}

export interface PetDisplayScaleListenerOptions {
  listen(handler: (request: unknown) => void): Promise<() => void>;
  emit(result: PetDisplayScaleResult): Promise<void>;
  apply(
    displayScale: number,
    commit: (ack: WindowSizeAck) => Promise<void>,
  ): Promise<WindowSizeAck>;
  commit(ack: WindowSizeAck): Promise<void>;
  diagnose?(stage: string, error: unknown): void;
}

export async function listenForPetDisplayScaleRequests(
  options: PetDisplayScaleListenerOptions,
): Promise<() => void> {
  const pending = new Map<string, number>();
  const completed = new Map<string, {
    requestedDisplayScale: number;
    result: PetDisplayScaleResult;
  }>();
  let queue = Promise.resolve();

  const diagnoseSafely = (stage: string, error: unknown): void => {
    try {
      options.diagnose?.(stage, error);
    } catch {
      // Diagnostics are observational and cannot break the request queue.
    }
  };

  const emitSafely = async (result: PetDisplayScaleResult): Promise<void> => {
    try {
      await options.emit(result);
    } catch (error) {
      diagnoseSafely("display-scale-result", error);
    }
  };
  const remember = (result: PetDisplayScaleResult): void => {
    completed.set(result.requestId, {
      requestedDisplayScale: result.requestedDisplayScale,
      result,
    });
    if (completed.size > 128) {
      const oldest = completed.keys().next().value as string | undefined;
      if (oldest) completed.delete(oldest);
    }
  };
  const process = async (request: PetDisplayScaleRequest): Promise<void> => {
    let result: PetDisplayScaleResult;
    try {
      const ack = await options.apply(request.displayScale, options.commit);
      result = {
        requestId: request.requestId,
        requestedDisplayScale: request.displayScale,
        ok: true,
        displayScale: ack.appliedScale,
        rect: ack.rect,
      };
    } catch (error) {
      result = {
        requestId: request.requestId,
        requestedDisplayScale: request.displayScale,
        ok: false,
        message: applyErrorMessage(error),
      };
    }
    if (pending.get(request.requestId) === request.displayScale) pending.delete(request.requestId);
    remember(result);
    await emitSafely(result);
  };
  const receive = (candidate: unknown): void => {
    if (!isPetDisplayScaleRequest(candidate)) {
      diagnoseSafely("display-scale-request", new TypeError("Invalid display scale request"));
      return;
    }
    const completedEntry = completed.get(candidate.requestId);
    if (completedEntry) {
      const result = completedEntry.requestedDisplayScale === candidate.displayScale
        ? completedEntry.result
        : scaleConflictResult(candidate);
      queue = queue.then(() => emitSafely(result));
      return;
    }
    const pendingScale = pending.get(candidate.requestId);
    if (pendingScale !== undefined) {
      if (pendingScale !== candidate.displayScale) {
        void emitSafely(scaleConflictResult(candidate));
      }
      return;
    }
    pending.set(candidate.requestId, candidate.displayScale);
    queue = queue.then(() => process(candidate));
  };

  return options.listen(receive);
}

function scaleConflictResult(request: PetDisplayScaleRequest): PetDisplayScaleResult {
  return {
    requestId: request.requestId,
    requestedDisplayScale: request.displayScale,
    ok: false,
    message: "requestId is already bound to a different display scale",
  };
}

export async function commitDisplayScalePreferences(
  preferences: ProbePreferences,
  ack: WindowSizeAck,
  save: (value: ProbePreferences) => Promise<void> = savePreferences,
): Promise<void> {
  const previous = { ...preferences };
  Object.assign(preferences, {
    displayScale: ack.appliedScale,
    x: Math.round(ack.rect.x),
    y: Math.round(ack.rect.y),
    width: Math.max(1, Math.round(ack.rect.width)),
    height: Math.max(1, Math.round(ack.rect.height)),
  });
  try {
    await save({ ...preferences });
  } catch (error) {
    Object.assign(preferences, previous);
    throw error;
  }
}

function applyErrorMessage(error: unknown): string {
  const primary = error instanceof Error ? error.message : String(error);
  const rollbacks = (error as WindowSizeApplyError | null)?.rollbackErrors ?? [];
  if (rollbacks.length === 0) return (primary || "Display scale request failed").slice(0, 2_048);
  const details = rollbacks.map(({ stage, error: rollbackError }) => (
    `${stage}: ${rollbackError instanceof Error ? rollbackError.message : String(rollbackError)}`
  ));
  return `${primary || "Display scale request failed"}; rollback failures: ${details.join(", ")}`
    .slice(0, 2_048);
}
