import "./styles.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PetStage } from "./runtime/pet-stage";
import { assetScan, probeFullscreen } from "./runtime/bridge";
import { parseManifestV1 } from "./runtime/manifest-schema";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("missing #app root");

try {
  const health = await assetScan();
  const activePetId = await invoke<string | null>("pet_get_active");
  const activeHealth = activePetId
    ? health.find((entry) => entry.petId === activePetId)
    : undefined;
  const degraded = activeHealth && activeHealth.status !== "healthy"
    ? { status: activeHealth.status }
    : undefined;
  let assets;
  if (activePetId && !degraded) {
    try {
      const manifestRaw = await invoke<string>("asset_manifest", { petId: activePetId });
      const manifest = parseManifestV1(JSON.parse(manifestRaw));
      const files = new Map(manifest.files.map((file) => [file.role, file.relativePath]));
      const bodyFile = files.get("body") ?? files.get("main") ?? "body.png";
      const bodyUrl = await invoke<string>("asset_file_b64", {
        petId: activePetId,
        file: bodyFile,
      });
      assets = {
        bodyUrl,
        eyeOpenUrl: bodyUrl,
        eyeClosedUrl: bodyUrl,
        accentUrl: bodyUrl,
        parts: manifest.parts,
        meshFeatures: manifest.meshFeatures,
      };
      if (manifest.assetType === "layered-v1") {
        const eyeOpenFile = files.get("eye-open");
        const eyeClosedFile = files.get("eye-closed");
        if (eyeOpenFile) {
          assets.eyeOpenUrl = await invoke<string>("asset_file_b64", {
            petId: activePetId,
            file: eyeOpenFile,
          });
        }
        if (eyeClosedFile) {
          assets.eyeClosedUrl = await invoke<string>("asset_file_b64", {
            petId: activePetId,
            file: eyeClosedFile,
          });
        }
      }
    } catch (error) {
      console.error("load active pet asset failed:", error);
    }
  }

  const stage = new PetStage();
  await stage.mount(root, degraded, assets);
} catch (error) {
  console.error("pet mount failed:", error);
}

// reload the pet window when another pet is activated from the settings window
void listen("pet-activated", () => window.location.reload()).catch((error) => {
  console.error("listen pet-activated failed:", error);
});

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
