import type { Live2DAssetTransport } from "../runtime-assets/live2d-asset-loader";

export const BUILTIN_LIVE2D_PET = {
  petId: "cat-a-standard-v1",
  manifestUrl: "/builtin-pets/cat-a-standard-v1/manifest.json",
  previewUrl: "/builtin-pets/cat-a-standard-v1/preview.png",
} as const;

const LEGACY_BUILTIN_LIVE2D_PET = {
  petId: "pet-live2d-v1",
  manifestUrl: "/builtin-pets/pet-live2d-v1/manifest.json",
  previewUrl: "/builtin-pets/pet-live2d-v1/preview.png",
} as const;

export type StartupPetSource =
  | { kind: "installed"; petId: string }
  | ({ kind: "builtin" } & (typeof BUILTIN_LIVE2D_PET | typeof LEGACY_BUILTIN_LIVE2D_PET));

interface BuiltinPetTransportOptions {
  manifestUrl: string;
  origin?: string;
  fetcher?: (input: RequestInfo | URL) => Promise<Response>;
}

export function selectStartupPetSource(activePetId: string | null): StartupPetSource {
  if (activePetId === LEGACY_BUILTIN_LIVE2D_PET.petId) {
    return { kind: "builtin", ...LEGACY_BUILTIN_LIVE2D_PET };
  }
  if (activePetId && activePetId !== BUILTIN_LIVE2D_PET.petId) return { kind: "installed", petId: activePetId };
  return { kind: "builtin", ...BUILTIN_LIVE2D_PET };
}

export function resolveBuiltinPetUrl(
  manifestUrl: string,
  relativePath: string,
  origin: string,
): string {
  return new URL(relativePath, new URL(manifestUrl, origin)).toString();
}

export function createBuiltinPetTransport(
  options: BuiltinPetTransportOptions,
): Live2DAssetTransport {
  const origin = options.origin ?? window.location.origin;
  const manifestUrl = new URL(options.manifestUrl, origin).toString();
  const fetcher = options.fetcher ?? ((input) => fetch(input));

  const read = async (url: string): Promise<Response> => {
    const response = await fetcher(url);
    if (!response.ok) {
      throw new Error(`built-in pet resource failed (${response.status}): ${url}`);
    }
    return response;
  };

  return {
    readManifest: async () => (await read(manifestUrl)).json(),
    readFile: async (_petId, relativePath) => {
      const resourceUrl = resolveBuiltinPetUrl(manifestUrl, relativePath, origin);
      const response = await read(resourceUrl);
      return new Uint8Array(await response.arrayBuffer());
    },
  };
}
