import { convertFileSrc } from "@tauri-apps/api/core";
import { normalizeAssetPath } from "../runtime-assets/live2d-manifest";

type AssetUrlConverter = (filePath: string, protocol: string) => string;

export function installedPetAssetUrl(
  petId: string,
  relativePath: string,
  converter: AssetUrlConverter = convertFileSrc,
): string {
  return converter(
    `${normalizeAssetPath(petId)}/assets/${normalizeAssetPath(relativePath)}`,
    "pet-asset",
  );
}
