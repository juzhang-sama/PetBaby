import "./styles.css";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow, availableMonitors, primaryMonitor } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { EffectOverlay } from "./runtime/effect-overlay";
import { applyHitRegion, loadPreferences, probeFullscreen, savePreferences } from "./runtime/bridge";
import { clampRectToWorkArea } from "./runtime/geometry";
import { alphaToRegionSpans } from "./runtime/hit-mask";
import { PetStage } from "./runtime/pet-stage";
import {
  type RendererDiagnostic,
} from "./runtime/pet-renderer-bootstrap";
import { WindowMotionController } from "./runtime/window-motion-controller";
import { isLive2DProbeMode, mountLive2DProbe } from "./runtime-live2d/probe";
import { isLive2DPreviewMode, mountLive2DPreview } from "./runtime-live2d/preview";
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
import { loadStartupRuntime } from "./runtime/startup-runtime-recovery";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");

function tracePetRuntime(message: string): void {
  void invoke("frontend_ping", { message: `pet-runtime: ${message}` }).catch(() => undefined);
}

if (isLive2DProbeMode(location.search)) {
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
      const descriptor = await invoke<RuntimePetDescriptor>("pet_prepare_switch", { petId });
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
    commit: (petId) => invoke("pet_commit_switch", { petId }),
    onRecovery: (petId, error) => {
      tracePetRuntime(`recovering-to-builtin: ${petId}: ${errorMessage(error)}`);
    },
  });
  initialRuntime = startup.runtime;
  if (startup.recoveredToBuiltin) tracePetRuntime("recovered-to-builtin");
  tracePetRuntime(`runtime-loaded: ${initialRuntime.kind()}`);
  slot = new PetRuntimeSlot(rendererRoot, initialRuntime);

  const windowMotion = new WindowMotionController({
    getPosition: async () => {
      const position = await petWindow.outerPosition();
      return { x: position.x, y: position.y };
    },
    setPosition: (position) => petWindow.setPosition(new PhysicalPosition(position.x, position.y)),
    persistPosition: async (position) => {
      const size = await petWindow.outerSize();
      Object.assign(preferences, {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
      });
      await savePreferences(preferences);
    },
  });

  const stage = new PetStage({
    renderer: slot,
    windowMotion,
    effects,
    refreshHitRegion,
    diagnose: (stageName, error) => diagnose({
      petId: slot.activePetId,
      manifestVersion: 0,
      stage: stageName === "window-motion" ? "window-motion" : "hit-region",
      message: errorMessage(error),
    }),
  });
  await stage.mount(rendererRoot);
  tracePetRuntime("stage-mounted");

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
    cancel: (requestId) => invoke("pet_cancel_switch", { requestId }),
    finish: (requestId) => invoke("pet_finish_switch", { requestId }),
    refreshHitRegion,
  });
  await listen<PetSwitchRequest>(PET_SWITCH_REQUEST, async ({ payload }) => {
    const result = await coordinator.switch(payload);
    await emitTo("settings", PET_SWITCH_RESULT, result);
  });
  tracePetRuntime("switch-listener-ready");

  let hiddenForFullscreen = false;
  window.setInterval(async () => {
    try {
      const snapshot = await probeFullscreen();
      if (snapshot.isFullscreen && !hiddenForFullscreen) {
        hiddenForFullscreen = true;
        stage.setVisibility(false);
        await petWindow.hide();
      } else if (!snapshot.isFullscreen && hiddenForFullscreen) {
        hiddenForFullscreen = false;
        stage.setVisibility(true);
        await petWindow.show();
      }
    } catch (error) {
      diagnose({
        petId: slot.activePetId,
        manifestVersion: 0,
        stage: "fullscreen",
        message: errorMessage(error),
      });
    }
  }, 750);
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
    const area = {
      x: monitor.position.x,
      y: monitor.position.y,
      width: monitor.size.width,
      height: monitor.size.height,
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
      const area = {
        x: monitor.position.x,
        y: monitor.position.y,
        width: monitor.size.width,
        height: monitor.size.height,
      };
      restored = saved.width > area.width * 0.95 || saved.height > area.height * 0.95
        ? { ...clampRectToWorkArea(saved, area, 64), ...defaultSize }
        : clampRectToWorkArea(saved, area, 64);
    }
  }

  Object.assign(preferences, restored);
  await petWindow.setSize(new PhysicalSize(restored.width, restored.height));
  await petWindow.setPosition(new PhysicalPosition(restored.x, restored.y));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
