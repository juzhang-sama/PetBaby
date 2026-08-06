import "./styles.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { PetStage } from "./runtime/pet-stage";
import { assetScan, probeFullscreen } from "./runtime/bridge";
import { mountLive2DProbe } from "./runtime-live2d/probe";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");

if (new URLSearchParams(location.search).get("live2dProbe") === "1") {
  const result = await mountLive2DProbe(root);
  root.dataset.live2dProbeResult = result.ok ? "ok" : result.reason;
  console.info("Live2D probe result", result);
}

try {
  const health = await assetScan();
  const unhealthy = health.find((entry) => entry.status !== "healthy");

  const activePetId = await invoke<string | null>("pet_get_active");
  const assets = activePetId
    ? {
        bodyUrl: `pet-asset://localhost/${activePetId}/assets/body.png`,
        eyeOpenUrl: `pet-asset://localhost/${activePetId}/assets/body.png`,
        eyeClosedUrl: `pet-asset://localhost/${activePetId}/assets/body.png`,
        accentUrl: `pet-asset://localhost/${activePetId}/assets/body.png`,
      }
    : undefined;

  const stage = new PetStage();
  await stage.mount(root, unhealthy ? { status: unhealthy.status } : undefined, assets);
} catch (error) {
  console.error("pet mount failed:", error);
}

let hiddenForFullscreen = false;
window.setInterval(async () => {
  const snapshot = await probeFullscreen();
  const petWindow = getCurrentWindow();
  if (snapshot.isFullscreen && !hiddenForFullscreen) {
    hiddenForFullscreen = true;
    await petWindow.hide();
  } else if (!snapshot.isFullscreen && hiddenForFullscreen) {
    hiddenForFullscreen = false;
    await petWindow.show();
  }
}, 750);
