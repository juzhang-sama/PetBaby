import "./styles.css";
import { LogicalPosition, LogicalSize, PhysicalPosition } from "@tauri-apps/api/dpi";
import {
  getCurrentWindow,
  availableMonitors,
  currentMonitor,
  primaryMonitor,
} from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { EffectOverlay } from "./runtime/effect-overlay";
import {
  applyHitRegion,
  createLogicalWindowGeometryPersistence,
  createLogicalWindowSizePort,
  listenForPetDisplayScaleRequests,
  loadPreferences,
  probeFullscreen,
  reconcileWindowVisibility,
  updateWindowFullscreen,
} from "./runtime/bridge";
import {
  PET_CALIBRATION_PREVIEW_REQUEST,
  PET_CALIBRATION_PREVIEW_RESULT,
  PET_DISPLAY_SCALE_REQUEST,
  PET_DISPLAY_SCALE_RESULT,
} from "./runtime/contracts";
import { clampRectToWorkArea } from "./runtime/geometry";
import { wireFullscreenProbeLoop } from "./runtime/fullscreen";
import { alphaToRegionSpans } from "./runtime/hit-mask";
import {
  PetStage,
  wirePetCalibrationRuntime,
} from "./runtime/pet-stage";
import type { PetCalibrationV1 } from "./runtime/pet-calibration";
import {
  type RendererDiagnostic,
} from "./runtime/pet-renderer-bootstrap";
import {
  runWithWindowMotionSuspended,
  WindowMotionController,
} from "./runtime/window-motion-controller";
import { isLive2DProbeMode, mountLive2DProbe } from "./runtime-live2d/probe";
import { isLive2DPreviewMode, mountLive2DPreview } from "./runtime-live2d/preview";
import {
  isCatMotionEvidenceMode,
  mountCatMotionEvidence,
} from "./runtime-live2d/cat-motion-evidence";
import { PetRuntimeSlot, type MountedPetRuntime } from "./runtime/pet-runtime-slot";
import { PetSwitchCoordinator } from "./runtime/pet-switch-coordinator";
import {
  PET_SWITCH_REQUEST,
  PET_SWITCH_RESULT,
  type PetSwitchRequest,
  type RuntimePetDescriptor,
} from "./runtime/pet-switch-protocol";
import { assertVisibleFrame } from "./runtime/render-surface-probe";
import { loadRuntimePet } from "./runtime/runtime-pet-loader";
import { finalizeStartupRecovery, loadStartupRuntime } from "./runtime/startup-runtime-recovery";
import { WindowSizeController } from "./runtime/window-size-controller";
import { wireWindowModeRuntime } from "./runtime/window-mode-runtime";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");

function tracePetRuntime(message: string): void {
  void invoke("frontend_ping", { message: `pet-runtime: ${message}` }).catch(() => undefined);
}

if (isCatMotionEvidenceMode(location.search)) {
  await mountCatMotionEvidence(root);
} else if (isLive2DProbeMode(location.search)) {
  const result = await mountLive2DProbe(root);
  root.dataset.live2dProbeResult = result.ok ? "ok" : result.reason;
  console.info("Live2D probe result", result);
} else if (isLive2DPreviewMode(location.search)) {
  await mountLive2DPreview(root);
} else {
  try {
    tracePetRuntime("mount-start");
    await mountPet(root);
    tracePetRuntime("mount-complete");
  } catch (error) {
    tracePetRuntime(`mount-failed: ${errorMessage(error)}`);
    console.error("Pet mount failed", {
      petId: "unknown",
      manifestVersion: 0,
      stage: "mount",
      message: errorMessage(error),
    } satisfies RendererDiagnostic);
  }
}

async function mountPet(appRoot: HTMLElement): Promise<void> {
  const preferences = await loadPreferences();
  tracePetRuntime("preferences-loaded");
  const petWindow = getCurrentWindow();
  await restoreWindowPlacement(petWindow, preferences);
  tracePetRuntime("window-placement-restored");

  const rendererRoot = document.createElement("div");
  rendererRoot.className = "pet-render-host";
  appRoot.replaceChildren(rendererRoot);
  const effects = new EffectOverlay(appRoot);

  let slot: PetRuntimeSlot;
  const refreshHitRegion = async (): Promise<void> => {
    const surface = slot.getHitSurface();
    const width = Math.max(1, rendererRoot.clientWidth || appRoot.clientWidth);
    const height = Math.max(1, rendererRoot.clientHeight || appRoot.clientHeight);
    const scratch = document.createElement("canvas");
    scratch.width = width;
    scratch.height = height;
    const context = scratch.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("2D canvas is unavailable for hit-mask extraction");
    context.clearRect(0, 0, width, height);
    context.drawImage(surface, 0, 0, width, height);
    const image = context.getImageData(0, 0, width, height);
    await applyHitRegion({
      canvasWidth: width,
      canvasHeight: height,
      scaleFactor: window.devicePixelRatio || 1,
      spans: alphaToRegionSpans(image.data, width, height, { alphaThreshold: 32, rowStep: 2 }),
    });
  };

  const diagnose = (diagnostic: RendererDiagnostic): void => {
    console.error("Pet renderer diagnostic", diagnostic);
  };

  const activePetId = await invoke<string>("pet_get_active");
  tracePetRuntime(`active-pet: ${activePetId}`);
  let initialRuntime: MountedPetRuntime | undefined;
  const startup = await loadStartupRuntime(activePetId, {
    prepare: async (petId) => {
      const descriptor = await invoke<RuntimePetDescriptor>("pet_prepare_startup", { petId });
      tracePetRuntime(`descriptor-prepared: ${descriptor.source}`);
      return descriptor;
    },
    load: (descriptor) => loadRuntimePet(
      descriptor,
      document.createElement("div"),
      undefined,
      {
        allowPreviewFallback: true,
        diagnose,
        onSurfaceChanged: async () => {
          if (initialRuntime && slot.refreshActiveSurface(initialRuntime)) await refreshHitRegion();
        },
      },
    ),
    commit: (petId) => finalizeStartupRecovery(activePetId, petId, {
      prepareSwitch: async (requestId, targetPetId) => {
        await invoke("pet_prepare_switch", { requestId, petId: targetPetId });
      },
      commit: (request) => invoke("pet_commit_switch", { ...request }),
      reconcileCommit: (previousPetId, request) => invoke("pet_reconcile_switch_commit", {
        previousPetId,
        ...request,
      }),
      cancel: (requestId) => invoke("pet_cancel_switch", { requestId }),
      finish: (requestId) => invoke("pet_finish_switch", { requestId }),
    }),
    onRecovery: (petId, error) => {
      tracePetRuntime(`recovering-to-builtin: ${petId}: ${errorMessage(error)}`);
    },
  });
  initialRuntime = startup.runtime;
  if (startup.recoveredToBuiltin) tracePetRuntime("recovered-to-builtin");
  if (startup.warning) tracePetRuntime(`startup-finalization-warning: ${startup.warning}`);
  tracePetRuntime(`runtime-loaded: ${initialRuntime.kind()}`);
  slot = new PetRuntimeSlot(rendererRoot, initialRuntime);

  const windowGeometryPersistence = createLogicalWindowGeometryPersistence({
    window: petWindow,
    preferences,
    diagnose: (_stage, error) => diagnose({
      petId: slot.activePetId,
      manifestVersion: 0,
      stage: "window-motion",
      message: errorMessage(error),
    }),
  });
  await petWindow.onResized(({ payload: size }) => {
    void windowGeometryPersistence.persist({ size });
  });

  const windowMotion = new WindowMotionController({
    getPosition: async () => {
      const position = await petWindow.outerPosition();
      return { x: position.x, y: position.y };
    },
    setPosition: (position) => petWindow.setPosition(new PhysicalPosition(position.x, position.y)),
    persistPosition: (position) => windowGeometryPersistence.persist({
      position: new PhysicalPosition(position.x, position.y),
    }),
  });

  const stage = new PetStage({
    renderer: slot,
    windowMotion,
    effects,
    refreshHitRegion,
    onFrameSample: (deltas) => tracePetRuntime(`frame-sample: deltas=${deltas.map((value) => value.toFixed(2)).join(",")}`),
    diagnose: (stageName, error) => diagnose({
      petId: slot.activePetId,
      manifestVersion: 0,
      stage: stageName === "window-motion" ? "window-motion" : "hit-region",
      message: errorMessage(error),
    }),
  });
  await stage.mount(rendererRoot);
  tracePetRuntime("stage-mounted");

  const windowModeWiring = await wireWindowModeRuntime({
    listen: async (event, handler) => listen<unknown>(event, ({ payload }) => handler(payload)),
    ready: () => invoke<number>("window_mode_runtime_ready"),
    ack: (requestId, cycle, phase) => invoke<boolean>("window_mode_runtime_ack", {
      requestId,
      cycle,
      phase,
    }),
    pause: () => stage.pauseWindowModeTransition(),
    resume: (effectiveVisible) => stage.resumeWindowModeTransition(effectiveVisible),
    abort: () => stage.abortWindowModeTransition(),
    diagnose: (stageName, error) => console.error("Window mode runtime diagnostic", {
      stage: stageName,
      message: errorMessage(error),
    }),
  });
  window.addEventListener("beforeunload", windowModeWiring.destroy, { once: true });
  tracePetRuntime("window-mode-listeners-ready");

  const calibrationWiring = await wirePetCalibrationRuntime({
    activePetId: () => slot.activePetId,
    load: (petId) => invoke<PetCalibrationV1>("pet_calibration_load", { petId }),
    setCalibration: (value) => stage.setCalibration(value),
    listen: async (handler) => listen<unknown>(
      PET_CALIBRATION_PREVIEW_REQUEST,
      ({ payload }) => handler(payload),
    ),
    emit: (result) => emitTo("settings", PET_CALIBRATION_PREVIEW_RESULT, result),
    previewFeedback: () => stage.previewFeedback(),
    diagnose: (stageName, error) => console.error("Pet calibration diagnostic", {
      petId: slot.activePetId,
      stage: stageName,
      message: errorMessage(error),
    }),
  });
  window.addEventListener("beforeunload", calibrationWiring.destroy, { once: true });
  tracePetRuntime("calibration-preview-listener-ready");

  const windowSize = new WindowSizeController(createLogicalWindowSizePort({
    window: petWindow,
    currentMonitor,
    resizeRenderer: async () => stage.refreshViewport(),
    refreshHitRegion,
  }));
  await listenForPetDisplayScaleRequests({
    listen: async (handler) => listen<unknown>(
      PET_DISPLAY_SCALE_REQUEST,
      ({ payload }) => handler(payload),
    ),
    emit: (result) => emitTo("settings", PET_DISPLAY_SCALE_RESULT, result),
    apply: (displayScale, commit) => runWithWindowMotionSuspended(
      windowMotion,
      () => windowGeometryPersistence.flushCurrentGeometry(),
      () => windowGeometryPersistence.runDisplayScaleTransaction(
        () => windowSize.apply(displayScale, commit),
      ),
    ),
    commit: (ack) => windowGeometryPersistence.commitDisplayScale(ack),
    diagnose: (stageName, error) => console.error("Display scale protocol diagnostic", {
      stage: stageName,
      message: errorMessage(error),
    }),
  });
  tracePetRuntime("display-scale-listener-ready");

  const coordinator = new PetSwitchCoordinator(slot, {
    prepare: (requestId, petId) => invoke("pet_prepare_switch", { requestId, petId }),
    load: async (descriptor, stagingRoot) => {
      let candidate: MountedPetRuntime | undefined;
      candidate = await loadRuntimePet(descriptor, stagingRoot, undefined, {
        diagnose,
        onSurfaceChanged: async () => {
          if (candidate && slot.refreshActiveSurface(candidate)) await refreshHitRegion();
        },
      });
      return candidate;
    },
    probe: assertVisibleFrame,
    commit: (request) => invoke("pet_commit_switch", { ...request }),
    rollbackCommit: (previousPetId, request) => invoke("pet_rollback_switch", {
      previousPetId,
      ...request,
    }),
    reconcileCommit: (previousPetId, request) => invoke("pet_reconcile_switch_commit", {
      previousPetId,
      ...request,
    }),
    abortCreation: (sessionId, error) => invoke("creation_abort_finalize", { sessionId, error }),
    cancel: (requestId) => invoke("pet_cancel_switch", { requestId }),
    finish: (requestId) => invoke("pet_finish_switch", { requestId }),
    refreshHitRegion,
  });
  await listen<PetSwitchRequest>(PET_SWITCH_REQUEST, async ({ payload }) => {
    const result = await coordinator.switch(payload);
    await calibrationWiring.afterPetSwitch(result);
    if (result.ok) stage.syncActiveRenderer();
    await emitTo("settings", PET_SWITCH_RESULT, result);
  });
  tracePetRuntime("switch-listener-ready");

  const fullscreenWiring = wireFullscreenProbeLoop({
    setInterval: (callback, delayMs) => window.setInterval(callback, delayMs),
    clearInterval: (id) => window.clearInterval(id),
    probe: probeFullscreen,
    update: updateWindowFullscreen,
    reconcile: reconcileWindowVisibility,
    diagnose: (error) => diagnose({
        petId: slot.activePetId,
        manifestVersion: 0,
        stage: "fullscreen",
        message: errorMessage(error),
    }),
  });
  window.addEventListener("beforeunload", fullscreenWiring.destroy, { once: true });
}

async function restoreWindowPlacement(
  petWindow: ReturnType<typeof getCurrentWindow>,
  preferences: Awaited<ReturnType<typeof loadPreferences>>,
): Promise<void> {
  const saved = {
    x: preferences.x,
    y: preferences.y,
    width: preferences.width,
    height: preferences.height,
  };
  const defaultSize = { width: 420, height: 520 };
  const monitors = await availableMonitors();
  let restored = saved;
  let anchored = false;

  for (const monitor of monitors) {
    const logicalPosition = monitor.workArea.position.toLogical(monitor.scaleFactor);
    const logicalSize = monitor.workArea.size.toLogical(monitor.scaleFactor);
    const area = {
      x: logicalPosition.x,
      y: logicalPosition.y,
      width: logicalSize.width,
      height: logicalSize.height,
    };
    const overlaps = saved.x < area.x + area.width && saved.x + saved.width > area.x
      && saved.y < area.y + area.height && saved.y + saved.height > area.y;
    if (!overlaps) continue;
    restored = saved.width > area.width * 0.95 || saved.height > area.height * 0.95
      ? { ...clampRectToWorkArea(saved, area, 64), ...defaultSize }
      : clampRectToWorkArea(saved, area, 64);
    anchored = true;
    break;
  }

  if (!anchored) {
    const monitor = await primaryMonitor();
    if (monitor) {
      const logicalPosition = monitor.workArea.position.toLogical(monitor.scaleFactor);
      const logicalSize = monitor.workArea.size.toLogical(monitor.scaleFactor);
      const area = {
        x: logicalPosition.x,
        y: logicalPosition.y,
        width: logicalSize.width,
        height: logicalSize.height,
      };
      restored = saved.width > area.width * 0.95 || saved.height > area.height * 0.95
        ? { ...clampRectToWorkArea(saved, area, 64), ...defaultSize }
        : clampRectToWorkArea(saved, area, 64);
    }
  }

  Object.assign(preferences, restored);
  await petWindow.setSize(new LogicalSize(restored.width, restored.height));
  await petWindow.setPosition(new LogicalPosition(restored.x, restored.y));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
