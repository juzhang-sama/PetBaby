import { invoke } from "@tauri-apps/api/core";
import { loadLive2DAsset, type Live2DAssetTransport } from "../runtime-assets/live2d-asset-loader";
import { createPetRendererRuntime, type PetRendererBootstrapOptions, type PetRendererRuntime } from "./pet-renderer-bootstrap";
import type { RuntimePetDescriptor } from "./pet-switch-protocol";
import type { MountedPetRuntime } from "./pet-runtime-slot";
import {
  createBuiltinPetTransport,
  resolveBuiltinPetUrl,
  selectStartupPetSource,
  type StartupPetSource,
} from "./startup-pet";

export interface RuntimePetLoaderPorts {
  readInstalledManifest(petId: string): Promise<unknown>;
  createBuiltinTransport(manifestUrl: string): Live2DAssetTransport;
  createRuntime(
    petId: string,
    manifest: unknown,
    options: PetRendererBootstrapOptions,
  ): Promise<PetRendererRuntime>;
}

const defaultPorts: RuntimePetLoaderPorts = {
  readInstalledManifest: (petId) => invoke("asset_manifest", { petId }),
  createBuiltinTransport: (manifestUrl) => createBuiltinPetTransport({ manifestUrl }),
  createRuntime: createPetRendererRuntime,
};

function builtinRuntimeOptions(
  root: HTMLElement,
  source: Extract<StartupPetSource, { kind: "builtin" }>,
  transport: Live2DAssetTransport,
): PetRendererBootstrapOptions {
  return {
    root,
    assetUrl: (_petId, relativePath) => resolveBuiltinPetUrl(
      source.manifestUrl,
      relativePath,
      window.location.origin,
    ),
    loadLive2DAsset: (petId, manifest) => loadLive2DAsset(petId, manifest, transport),
  };
}

export async function loadRuntimePet(
  descriptor: RuntimePetDescriptor,
  root: HTMLElement,
  ports: RuntimePetLoaderPorts = defaultPorts,
): Promise<MountedPetRuntime> {
  if (descriptor.source === "installed") {
    const manifest = await ports.readInstalledManifest(descriptor.petId);
    return {
      petId: descriptor.petId,
      ...(await ports.createRuntime(descriptor.petId, manifest, { root })),
    };
  }

  const source = selectStartupPetSource(descriptor.petId);
  if (source.kind !== "builtin") throw new Error(`missing built-in pet source: ${descriptor.petId}`);
  const transport = ports.createBuiltinTransport(source.manifestUrl);
  const manifest = await transport.readManifest(descriptor.petId);
  return {
    petId: descriptor.petId,
    ...(await ports.createRuntime(
      descriptor.petId,
      manifest,
      builtinRuntimeOptions(root, source, transport),
    )),
  };
}
