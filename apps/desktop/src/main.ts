import "./styles.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PetStage } from "./runtime/pet-stage";
import { assetScan, probeFullscreen } from "./runtime/bridge";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");

const health = await assetScan();
const unhealthy = health.find((entry) => entry.status !== "healthy");
await new PetStage().mount(root, unhealthy ? { status: unhealthy.status } : undefined);

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
