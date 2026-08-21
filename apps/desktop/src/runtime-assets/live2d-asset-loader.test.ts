import { afterEach, describe, expect, it, vi } from "vitest";
import { loadLive2DAsset, photoAvatarManifestSha256 } from "./live2d-asset-loader";
import type { RuntimeAssetManifestV2 } from "./live2d-manifest";
import type { RuntimeAssetManifestV4 } from "./cat-character-manifest";
import type { RuntimeAssetManifestV5 } from "./cat-spatial-manifest";
import { motionSpatialProfileForTest } from "./cat-motion-spatial-profile-test-fixtures";

const model = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
const preview = new TextEncoder().encode("preview");
const manifest: RuntimeAssetManifestV2 = {
  schemaVersion: 2, renderer: "live2d-v1", petId: "pet-a", variantId: "v1",
  modelEntry: "model.model3.json", previewImage: "preview.png",
  files: [
    { role: "model", relativePath: "model.model3.json", sha256: "3d8da9c29b013f27dac037b04727f79eeb72029654714f704de74e9679f681d6" },
    { role: "preview", relativePath: "preview.png", sha256: "5975cf1bba432391c94667f5886225f69377c0aa8b9fa21fddfb21c89bcf9092" },
  ],
  semantics: { motions: {}, expressions: {}, hitAreas: {}, parameters: {} },
  license: { id: "test", author: "Test", source: "https://example.com", commercialUse: true, redistributable: false },
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe("loadLive2DAsset", () => {
  it("validates every digest before creating object URLs", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:model");
    const transport = { readManifest: vi.fn(async () => manifest), readFile: vi.fn(async (_petId: string, path: string) => path === "model.model3.json" ? model : new TextEncoder().encode("corrupt")) };
    await expect(loadLive2DAsset("pet-a", manifest, transport)).rejects.toThrow(/sha256/i);
    expect(create).not.toHaveBeenCalled();
    create.mockRestore();
  });

  it("disposes every URL after a successful load", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => `blob:${(blob as Blob).size}`);
    const transport = { readManifest: vi.fn(async () => manifest), readFile: vi.fn(async (_petId: string, path: string) => path === "model.model3.json" ? model : preview) };
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const asset = await loadLive2DAsset("pet-a", manifest, transport);
    expect(asset.modelUrl).toMatch(/^blob:/);
    expect(asset.previewUrl).toBe("blob:7");
    asset.dispose();
    asset.dispose();
    expect(revoke).toHaveBeenCalledTimes(2);
    create.mockRestore();
    revoke.mockRestore();
  });

  it("revokes URLs already created when later URL creation fails", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValueOnce("blob:model").mockImplementationOnce(() => { throw new Error("URL failed"); });
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const transport = { readManifest: vi.fn(async () => manifest), readFile: vi.fn(async (_petId: string, path: string) => path === "model.model3.json" ? model : preview) };
    await expect(loadLive2DAsset("pet-a", manifest, transport)).rejects.toThrow("URL failed");
    expect(revoke).toHaveBeenCalledWith("blob:model");
    create.mockRestore();
    revoke.mockRestore();
  });

  it("publishes every declared resource and rewrites model references to object URLs", async () => {
    const encoder = new TextEncoder();
    const resources = new Map<string, Uint8Array>([
      ["model.model3.json", encoder.encode(JSON.stringify({
        Version: 3,
        FileReferences: {
          Moc: "pet.moc3",
          Textures: ["textures/body.png"],
          Motions: { Idle: [{ File: "motions/idle.motion3.json" }] },
        },
      }))],
      ["pet.moc3", encoder.encode("moc")],
      ["textures/body.png", encoder.encode("texture")],
      ["motions/idle.motion3.json", encoder.encode("motion")],
      ["preview.png", encoder.encode("preview")],
    ]);
    const files = await Promise.all([...resources].map(async ([relativePath, bytes]) => ({
      role: relativePath === "preview.png" ? "preview" : "model-resource",
      relativePath,
      sha256: await digest(bytes),
    })));
    const packageManifest: RuntimeAssetManifestV2 = { ...manifest, files };
    const blobs = new Map<string, Blob>();
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const create = vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => {
      const url = `blob:${blobs.size}`;
      blobs.set(url, blob as Blob);
      return url;
    });
    const transport = {
      readManifest: vi.fn(async () => packageManifest),
      readFile: vi.fn(async (_petId: string, path: string) => resources.get(path)!),
    };

    const asset = await loadLive2DAsset("pet-a", packageManifest, transport);
    const publishedModel = JSON.parse(await blobs.get(asset.modelUrl)!.text());

    expect(create).toHaveBeenCalledTimes(resources.size);
    expect(publishedModel.FileReferences.Moc).toMatch(/^blob:/);
    expect(publishedModel.FileReferences.Textures[0]).toMatch(/^blob:/);
    expect(publishedModel.FileReferences.Motions.Idle[0].File).toMatch(/^blob:/);
    asset.dispose();
    asset.dispose();
    expect(revoke).toHaveBeenCalledTimes(resources.size);
    create.mockRestore();
  });

  it("rejects a Tauri manifest that differs from the caller manifest", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:unused");
    const changed = {
      ...manifest,
      semantics: { ...manifest.semantics, expressions: { happy: "Happy" } },
    };
    const transport = {
      readManifest: vi.fn(async () => changed),
      readFile: vi.fn(async () => model),
    };

    await expect(loadLive2DAsset("pet-a", manifest, transport)).rejects.toThrow(/manifest mismatch/i);
    expect(transport.readFile).not.toHaveBeenCalled();
    expect(create).not.toHaveBeenCalled();
    create.mockRestore();
  });

  it("creates blobs from the verified Uint8Array view only", async () => {
    const modelJson = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
    const backing = new TextEncoder().encode("prefix-preview-suffix");
    const previewView = backing.subarray(7, 14);
    const exactManifest: RuntimeAssetManifestV2 = {
      ...manifest,
      files: [
        { role: "model", relativePath: manifest.modelEntry, sha256: await digest(modelJson) },
        { role: "preview", relativePath: manifest.previewImage, sha256: await digest(previewView) },
      ],
    };
    const blobs = new Map<string, Blob>();
    const create = vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => {
      const url = `blob:${blobs.size}`;
      blobs.set(url, blob as Blob);
      return url;
    });
    const transport = {
      readManifest: vi.fn(async () => exactManifest),
      readFile: vi.fn(async (_petId: string, path: string) => path === manifest.modelEntry ? modelJson : previewView),
    };

    const asset = await loadLive2DAsset("pet-a", exactManifest, transport);

    expect(blobs.get(asset.previewUrl)?.size).toBe(previewView.byteLength);
    expect(await blobs.get(asset.previewUrl)?.text()).toBe("preview");
    asset.dispose();
    create.mockRestore();
  });

  it("loads a verified v4 cat package and preserves its independent controls", async () => {
    const modelJson = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
    const previewBytes = new TextEncoder().encode("cat-preview");
    const blinkOverlayBytes = new TextEncoder().encode("cat-blink-overlay");
    const catManifest: RuntimeAssetManifestV4 = {
      schemaVersion: 4,
      renderer: "cat-live2d-v1",
      petId: "cat-a",
      variantId: "standard-v1",
      skeletonVersion: "cat-a-live2d-v1",
      modelEntry: "cat.model3.json",
      previewImage: "preview.png",
      files: [
        { role: "model", relativePath: "cat.model3.json", sha256: await digest(modelJson) },
        { role: "preview", relativePath: "preview.png", sha256: await digest(previewBytes) },
        { role: "blink-overlay", relativePath: "overlays/blink-eyelids.png", sha256: await digest(blinkOverlayBytes) },
      ],
      motions: Object.fromEntries([
        "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
        "sleepy-yawn", "half-stand-stretch",
      ].map((name) => [name, { group: name, index: 0 }])) as RuntimeAssetManifestV4["motions"],
      parameters: Object.fromEntries([
        "eyeOpenLeft", "eyeOpenRight", "eyeBallX", "eyeBallY", "earLeft", "earRight",
        "tailAngle", "tailCurl", "tailTip", "bodyBreath", "bodyStretch", "mouthOpen",
      ].map((name) => [name, `Param-${name}`])) as RuntimeAssetManifestV4["parameters"],
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates: Object.fromEntries(
        ["left", "right", "top", "bottom"].map((name) => [name, {
          group: `edge-tail-${name}`, index: 0, tailArtMesh: "ArtMeshTail",
        }]),
      ) as RuntimeAssetManifestV4["edgeTailStates"],
      license: { id: "project", author: "PetBaby", source: "project", commercialUse: true, redistributable: true },
    };
    vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => `blob:${(blob as Blob).size}`);
    const transport = {
      readManifest: vi.fn(async () => catManifest),
      readFile: vi.fn(async (_petId: string, path: string) => path === catManifest.modelEntry
        ? modelJson
        : path === catManifest.previewImage
          ? previewBytes
          : blinkOverlayBytes),
    };

    const asset = await loadLive2DAsset("cat-a", catManifest, transport);

    expect(asset.semantics.motions["tail-idle"]).toEqual({ group: "tail-idle", index: 0 });
    expect(asset.semantics.parameters.eyeOpenLeft).toBe("Param-eyeOpenLeft");
    expect(asset.semantics.parameters.eyeOpenRight).toBe("Param-eyeOpenRight");
    expect(asset.semantics.parameters.tailTip).toBe("Param-tailTip");
    expect(asset.semantics.hitAreas.edgeTail).toBe("ArtMeshTail");
    expect(asset.blinkOverlayUrl).toBe(`blob:${blinkOverlayBytes.byteLength}`);
    expect(asset.motionSpatialProfile).toBeUndefined();
    asset.dispose();
  });

  it("loads a verified v5 spatial cat package as a cat Live2D asset", async () => {
    const modelJson = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
    const previewBytes = new TextEncoder().encode("cat-preview");
    const profile = motionSpatialProfileForTest();
    const profileBytes = new TextEncoder().encode(JSON.stringify(profile));
    const spatialManifest: RuntimeAssetManifestV5 = {
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "cat-a",
      variantId: "balanced-v1",
      skeletonVersion: "cat-a-live2d-v1",
      bodyModuleId: "body-balanced-v1",
      modelEntry: "cat.model3.json",
      previewImage: "preview.png",
      motionSpatialProfile: "profiles/body-balanced.json",
      files: [
        { role: "model", relativePath: "cat.model3.json", sha256: await digest(modelJson) },
        { role: "preview", relativePath: "preview.png", sha256: await digest(previewBytes) },
        { role: "motion-spatial-profile", relativePath: "profiles/body-balanced.json", sha256: await digest(profileBytes) },
      ],
      motions: Object.fromEntries([
        "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
        "sleepy-yawn", "half-stand-stretch",
      ].map((name) => [name, { group: name, index: 0 }])) as RuntimeAssetManifestV5["motions"],
      parameters: Object.fromEntries([
        "eyeOpenLeft", "eyeOpenRight", "eyeBallX", "eyeBallY", "earLeft", "earRight",
        "tailAngle", "tailCurl", "tailTip", "bodyBreath", "bodyStretch", "mouthOpen",
      ].map((name) => [name, `Param-${name}`])) as RuntimeAssetManifestV5["parameters"],
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates: Object.fromEntries(
        ["left", "right", "top", "bottom"].map((name) => [name, {
          group: `edge-tail-${name}`, index: 0, tailArtMesh: "ArtMeshTail",
        }]),
      ) as RuntimeAssetManifestV5["edgeTailStates"],
      license: { id: "project", author: "PetBaby", source: "project", commercialUse: true, redistributable: true },
    };
    const bytesByPath = new Map<string, Uint8Array>([
      [spatialManifest.modelEntry, modelJson],
      [spatialManifest.previewImage, previewBytes],
      [spatialManifest.motionSpatialProfile, profileBytes],
    ]);
    vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => `blob:${(blob as Blob).size}`);
    const transport = {
      readManifest: vi.fn(async () => JSON.parse(JSON.stringify(spatialManifest)) as unknown),
      readFile: vi.fn(async (_petId: string, path: string) => bytesByPath.get(path)!),
    };

    const asset = await loadLive2DAsset("cat-a", spatialManifest, transport);

    expect(asset.semantics.motions["tail-idle"]).toEqual({ group: "tail-idle", index: 0 });
    expect(asset.semantics.parameters.tailTip).toBe("Param-tailTip");
    expect(asset.motionSpatialProfile).toEqual(profile);
    expect(Object.isFrozen(asset.motionSpatialProfile)).toBe(true);
    expect(Object.isFrozen(asset.motionSpatialProfile?.amplitude)).toBe(true);
    asset.dispose();
  });

  it.each([
    ["unknown", "body-unknown-v1"],
    ["mismatched", "body-slender-v1"],
  ])("rejects a verified v5 spatial profile that is %s before publishing object URLs", async (
    _case,
    profileBodyModuleId,
  ) => {
    const modelJson = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
    const previewBytes = new TextEncoder().encode("cat-preview");
    const invalidProfile = {
      ...motionSpatialProfileForTest(),
      bodyModuleId: profileBodyModuleId,
    };
    const profileBytes = new TextEncoder().encode(JSON.stringify(invalidProfile));
    const spatialManifest: RuntimeAssetManifestV5 = {
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "cat-a",
      variantId: "balanced-v1",
      skeletonVersion: "cat-a-live2d-v1",
      bodyModuleId: "body-balanced-v1",
      modelEntry: "cat.model3.json",
      previewImage: "preview.png",
      motionSpatialProfile: "profiles/body-balanced.json",
      files: [
        { role: "model", relativePath: "cat.model3.json", sha256: await digest(modelJson) },
        { role: "preview", relativePath: "preview.png", sha256: await digest(previewBytes) },
        {
          role: "motion-spatial-profile",
          relativePath: "profiles/body-balanced.json",
          sha256: await digest(profileBytes),
        },
      ],
      motions: Object.fromEntries([
        "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
        "sleepy-yawn", "half-stand-stretch",
      ].map((name) => [name, { group: name, index: 0 }])) as RuntimeAssetManifestV5["motions"],
      parameters: Object.fromEntries([
        "eyeOpenLeft", "eyeOpenRight", "eyeBallX", "eyeBallY", "earLeft", "earRight",
        "tailAngle", "tailCurl", "tailTip", "bodyBreath", "bodyStretch", "mouthOpen",
      ].map((name) => [name, `Param-${name}`])) as RuntimeAssetManifestV5["parameters"],
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates: Object.fromEntries(
        ["left", "right", "top", "bottom"].map((name) => [name, {
          group: `edge-tail-${name}`, index: 0, tailArtMesh: "ArtMeshTail",
        }]),
      ) as RuntimeAssetManifestV5["edgeTailStates"],
      license: {
        id: "project",
        author: "PetBaby",
        source: "project",
        commercialUse: true,
        redistributable: true,
      },
    };
    const bytesByPath = new Map<string, Uint8Array>([
      [spatialManifest.modelEntry, modelJson],
      [spatialManifest.previewImage, previewBytes],
      [spatialManifest.motionSpatialProfile, profileBytes],
    ]);
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:unused");
    const transport = {
      readManifest: vi.fn(async () => JSON.parse(JSON.stringify(spatialManifest)) as unknown),
      readFile: vi.fn(async (_petId: string, path: string) => bytesByPath.get(path)!),
    };

    await expect(loadLive2DAsset("cat-a", spatialManifest, transport)).rejects.toThrow(/bodyModuleId/i);
    expect(create).not.toHaveBeenCalled();
  });

  it("hashes the v5 photo-avatar manifest using the builder's pretty JSON bytes", async () => {
    const modelJson = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
    const previewBytes = new TextEncoder().encode("cat-preview");
    const profileBytes = new TextEncoder().encode(JSON.stringify(motionSpatialProfileForTest()));
    const photoManifest: RuntimeAssetManifestV5 = {
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "photo-avatar-session-1-1",
      variantId: "photo-avatar-session-1-1",
      skeletonVersion: "cat-a-live2d-v1",
      bodyModuleId: "body-balanced-v1",
      modelEntry: "cat.model3.json",
      previewImage: "texture.png",
      motionSpatialProfile: "motion-spatial-profile.json",
      files: [
        { role: "model3", relativePath: "cat.model3.json", sha256: await digest(modelJson) },
        { role: "motion-spatial-profile", relativePath: "motion-spatial-profile.json", sha256: await digest(profileBytes) },
        { role: "texture", relativePath: "texture.png", sha256: await digest(previewBytes) },
      ],
      motions: Object.fromEntries([
        "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
        "sleepy-yawn", "half-stand-stretch",
      ].map((name) => [name, { group: name, index: 0 }])) as RuntimeAssetManifestV5["motions"],
      parameters: Object.fromEntries([
        "eyeOpenLeft", "eyeOpenRight", "eyeBallX", "eyeBallY", "earLeft", "earRight",
        "tailAngle", "tailCurl", "tailTip", "bodyBreath", "bodyStretch", "mouthOpen",
      ].map((name) => [name, `Param-${name}`])) as RuntimeAssetManifestV5["parameters"],
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates: Object.fromEntries(
        ["left", "right", "top", "bottom"].map((name) => [name, {
          group: `edge-tail-${name}`, index: 0, tailArtMesh: "ArtMeshTail",
        }]),
      ) as RuntimeAssetManifestV5["edgeTailStates"],
      license: { id: "project", author: "PetBaby", source: "project", commercialUse: true, redistributable: true },
    };
    const builderBytes = new TextEncoder().encode(JSON.stringify({
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "photo-avatar-session-1-1",
      variantId: "photo-avatar-session-1-1",
      skeletonVersion: "cat-a-live2d-v1",
      bodyModuleId: "body-balanced-v1",
      modelEntry: "cat.model3.json",
      previewImage: "texture.png",
      motionSpatialProfile: "motion-spatial-profile.json",
      files: photoManifest.files,
      motions: Object.fromEntries(Object.entries(photoManifest.motions).sort(([left], [right]) => left.localeCompare(right))),
      parameters: Object.fromEntries(Object.entries(photoManifest.parameters).sort(([left], [right]) => left.localeCompare(right))),
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates: Object.fromEntries(Object.entries(photoManifest.edgeTailStates).sort(([left], [right]) => left.localeCompare(right))),
      license: photoManifest.license,
    }, null, 2));

    expect(await photoAvatarManifestSha256(photoManifest)).toBe(await digest(builderBytes));
  });

  it("rejects a v5 motion spatial profile with a mismatched digest before publishing URLs", async () => {
    const modelJson = new TextEncoder().encode(JSON.stringify({ Version: 3, FileReferences: {} }));
    const previewBytes = new TextEncoder().encode("cat-preview");
    const profileBytes = new TextEncoder().encode(JSON.stringify(motionSpatialProfileForTest()));
    const manifest: RuntimeAssetManifestV5 = {
      schemaVersion: 5,
      renderer: "cat-spatial-live2d-v1",
      petId: "cat-a",
      variantId: "balanced-v1",
      skeletonVersion: "cat-a-live2d-v1",
      bodyModuleId: "body-balanced-v1",
      modelEntry: "cat.model3.json",
      previewImage: "preview.png",
      motionSpatialProfile: "profiles/body-balanced.json",
      files: [
        { role: "model", relativePath: "cat.model3.json", sha256: await digest(modelJson) },
        { role: "preview", relativePath: "preview.png", sha256: await digest(previewBytes) },
        { role: "motion-spatial-profile", relativePath: "profiles/body-balanced.json", sha256: "0".repeat(64) },
      ],
      motions: Object.fromEntries([
        "breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy",
        "sleepy-yawn", "half-stand-stretch",
      ].map((name) => [name, { group: name, index: 0 }])) as RuntimeAssetManifestV5["motions"],
      parameters: Object.fromEntries([
        "eyeOpenLeft", "eyeOpenRight", "eyeBallX", "eyeBallY", "earLeft", "earRight",
        "tailAngle", "tailCurl", "tailTip", "bodyBreath", "bodyStretch", "mouthOpen",
      ].map((name) => [name, `Param-${name}`])) as RuntimeAssetManifestV5["parameters"],
      hitAreas: { body: "ArtMeshBody", edgeTail: "ArtMeshTail" },
      edgeTailStates: Object.fromEntries(
        ["left", "right", "top", "bottom"].map((name) => [name, {
          group: `edge-tail-${name}`, index: 0, tailArtMesh: "ArtMeshTail",
        }]),
      ) as RuntimeAssetManifestV5["edgeTailStates"],
      license: { id: "project", author: "PetBaby", source: "project", commercialUse: true, redistributable: true },
    };
    const files = new Map<string, Uint8Array>([
      [manifest.modelEntry, modelJson],
      [manifest.previewImage, previewBytes],
      [manifest.motionSpatialProfile, profileBytes],
    ]);
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:unexpected");

    await expect(loadLive2DAsset("cat-a", manifest, {
      readManifest: async () => manifest,
      readFile: async (_petId, path) => files.get(path)!,
    })).rejects.toThrow(/sha256 mismatch: profiles\/body-balanced\.json/i);
    expect(create).not.toHaveBeenCalled();
  });
});

async function digest(bytes: Uint8Array): Promise<string> {
  const copy = Uint8Array.from(bytes);
  const value = await crypto.subtle.digest("SHA-256", copy);
  return [...new Uint8Array(value)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
