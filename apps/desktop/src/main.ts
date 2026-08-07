import "./styles.css";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow, availableMonitors, primaryMonitor } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { EffectOverlay } from "./runtime/effect-overlay";
import { applyHitRegion, loadPreferences, probeFullscreen, savePreferences } from "./runtime/bridge";
import { clampRectToWorkArea } from "./runtime/geometry";
import { alphaToRegionSpans } from "./runtime/hit-mask";
import { PetStage } from "./runtime/pet-stage";
import {
  createPetRendererRuntime,
  createStaticPngRuntime,
  type PetRendererRuntime,
  type RendererDiagnostic,
} from "./runtime/pet-renderer-bootstrap";
import { WindowMotionController } from "./runtime/window-motion-controller";
import { isLive2DProbeMode, mountLive2DProbe } from "./runtime-live2d/probe";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");

if (isLive2DProbeMode(location.search)) {
  const result = await mountLive2DProbe(root);
  root.dataset.live2dProbeResult = result.ok ? "ok" : result.reason;
  console.info("Live2D probe result", result);
} else {
  try {
    await mountPet(root);
  } catch (error) {
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
  const petWindow = getCurrentWindow();
  await restoreWindowPlacement(petWindow, preferences);

  const rendererRoot = document.createElement("div");
  rendererRoot.className = "pet-render-host";
  appRoot.replaceChildren(rendererRoot);
  const effects = new EffectOverlay(appRoot);

  let runtime: PetRendererRuntime;
  const refreshHitRegion = async (): Promise<void> => {
    const surface = runtime.getSurface();
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

  const activePetId = await invoke<string | null>("pet_get_active");
  let activeManifestVersion = 0;
  if (activePetId) {
    try {
      const manifestJson = await invoke<unknown>("asset_manifest", { petId: activePetId });
      activeManifestVersion = manifestVersionOf(manifestJson);
      runtime = await createPetRendererRuntime(activePetId, manifestJson, {
        root: rendererRoot,
        diagnose,
        onSurfaceChanged: refreshHitRegion,
      });
    } catch (error) {
      diagnose({
        petId: activePetId,
        manifestVersion: activeManifestVersion,
        stage: "manifest-load",
        message: errorMessage(error),
      });
      runtime = await createStaticPngRuntime(
        `pet-asset://localhost/${activePetId}/assets/body.png`,
        { root: rendererRoot, diagnose },
      );
    }
  } else {
    runtime = await createStaticPngRuntime("/test-assets/layered/body.png", { root: rendererRoot });
  }

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
    renderer: runtime.host,
    windowMotion,
    effects,
    refreshHitRegion,
    diagnose: (stageName, error) => diagnose({
      petId: activePetId ?? "none",
      manifestVersion: activeManifestVersion,
      stage: stageName === "window-motion" ? "window-motion" : "hit-region",
      message: errorMessage(error),
    }),
  });
  await stage.mount(rendererRoot);

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
        petId: activePetId ?? "none",
        manifestVersion: activeManifestVersion,
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

function manifestVersionOf(value: unknown): number {
  if (typeof value === "object" && value !== null && "schemaVersion" in value) {
    const version = (value as { schemaVersion?: unknown }).schemaVersion;
    return typeof version === "number" ? version : 0;
  }
  return 0;
}
