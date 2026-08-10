import type { PetRenderAsset } from "./pet-renderer";
import {
  parseAnimatedImageManifest,
  parseMotionProfile,
  type RuntimeAssetManifestV3,
} from "./animated-image-manifest";

async function defaultFetchJson(url: string): Promise<unknown> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`failed to fetch motion profile: HTTP ${response.status}`);
  return response.json();
}

export async function loadAnimatedImageAsset(
  petId: string,
  manifest: RuntimeAssetManifestV3,
  assetUrl: (petId: string, path: string) => string,
  fetchJson: (url: string) => Promise<unknown> = defaultFetchJson,
): Promise<Extract<PetRenderAsset, { kind: "animated-image" }>> {
  const parsedManifest = parseAnimatedImageManifest(manifest);
  if (parsedManifest.petId !== petId) throw new Error("manifest mismatch");
  const motionProfile = parseMotionProfile(
    await fetchJson(assetUrl(petId, parsedManifest.motionProfile)),
  );
  return {
    kind: "animated-image",
    imageUrl: assetUrl(petId, parsedManifest.image),
    motionProfile,
  };
}
