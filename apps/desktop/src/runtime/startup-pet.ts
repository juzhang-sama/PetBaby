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

// ⚠️ 01/02/03 已下线（2026-09-21）：它们是 schema 7 帧序列，但素材来自**像素风**
// （variantId = pixel-mid-simple-v1，布局是 body.png + actions/），与新产线不一致。
// 目录已挪到 `_archive-builtin-pets-20260921/`，别再把它们加回来。
export const BUILTIN_PIXEL_PETS = [
  // 绿幕抠像路线产物（AI 视频 -> 抠像 -> 单视频循环）。
  // baseImage = 循环首帧，previewUrl 指向它（无独立 body.png）。帧已转 WebP 瘦身。
  {
    petId: "04-warm-brown-tabby",
    manifestUrl: "/builtin-pets/04-warm-brown-tabby/manifest.json",
    previewUrl: "/builtin-pets/04-warm-brown-tabby/frames/idle-combo/f0000.webp",
  },
  // 建国2（银渐层）：通用性验证，证明管线不依赖毛色。2026-09-03 接入。
  {
    petId: "05-silver-tabby",
    manifestUrl: "/builtin-pets/05-silver-tabby/manifest.json",
    previewUrl: "/builtin-pets/05-silver-tabby/frames/idle-combo/f0000.webp",
  },
  // 果冻（短毛猫）：第一只「真实照片 → AI 视频 → 抠像 → 帧序列」端到端跑通的宠物。
  // 2026-09-12 接入（photo → master → green-screen first frame → Seedance 12s → matting → 四判据全 PASS）。
  {
    petId: "06-guodong",
    manifestUrl: "/builtin-pets/06-guodong/manifest.json",
    previewUrl: "/builtin-pets/06-guodong/frames/idle-combo/f0000.webp",
  },
  // 长毛猫 / 短毛犬 / 金毛：同批 4 张真实照片的另外 3 只（2026-09-12 接入，证明跨物种跨毛长通用）。
  {
    petId: "07-long-hair-cat",
    manifestUrl: "/builtin-pets/07-long-hair-cat/manifest.json",
    previewUrl: "/builtin-pets/07-long-hair-cat/frames/idle-combo/f0000.webp",
  },
  {
    petId: "08-short-hair-dog",
    manifestUrl: "/builtin-pets/08-short-hair-dog/manifest.json",
    previewUrl: "/builtin-pets/08-short-hair-dog/frames/idle-combo/f0000.webp",
  },
  {
    petId: "09-golden-retriever",
    manifestUrl: "/builtin-pets/09-golden-retriever/manifest.json",
    previewUrl: "/builtin-pets/09-golden-retriever/frames/idle-combo/f0000.webp",
  },
] as const;

export type BuiltinPixelPet = (typeof BUILTIN_PIXEL_PETS)[number];

export type StartupPetSource =
  | { kind: "installed"; petId: string }
  | ({ kind: "builtin" } & (typeof BUILTIN_LIVE2D_PET | typeof LEGACY_BUILTIN_LIVE2D_PET | BuiltinPixelPet));

interface BuiltinPetTransportOptions {
  manifestUrl: string;
  origin?: string;
  fetcher?: (input: RequestInfo | URL) => Promise<Response>;
}

export function selectStartupPetSource(activePetId: string | null): StartupPetSource {
  if (activePetId === LEGACY_BUILTIN_LIVE2D_PET.petId) {
    return { kind: "builtin", ...LEGACY_BUILTIN_LIVE2D_PET };
  }
  const pixelPet = BUILTIN_PIXEL_PETS.find((pet) => pet.petId === activePetId);
  if (pixelPet) return { kind: "builtin", ...pixelPet };
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
