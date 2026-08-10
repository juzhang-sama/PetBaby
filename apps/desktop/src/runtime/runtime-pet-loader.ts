import { invoke } from "@tauri-apps/api/core";
import { loadLive2DAsset, type Live2DAssetTransport } from "../runtime-assets/live2d-asset-loader";
import {
  createPetRendererRuntime,
  createStaticPngRuntime,
  type PetRendererBootstrapOptions,
  type PetRendererRuntime,
  type RendererDiagnostic,
} from "./pet-renderer-bootstrap";
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
  createPreviewRuntime(imageUrl: string, options: PetRendererBootstrapOptions): Promise<PetRendererRuntime>;
}

export interface RuntimePetLoadOptions {
  allowPreviewFallback?: boolean;
  diagnose?: (diagnostic: RendererDiagnostic) => void;
  onSurfaceChanged?: () => void | Promise<void>;
}

const defaultPorts: RuntimePetLoaderPorts = {
  readInstalledManifest: (petId) => invoke("asset_manifest", { petId }),
  createBuiltinTransport: (manifestUrl) => createBuiltinPetTransport({ manifestUrl }),
  createRuntime: createPetRendererRuntime,
  createPreviewRuntime: createStaticPngRuntime,
};

function builtinRuntimeOptions(
  root: HTMLElement,
  source: Extract<StartupPetSource, { kind: "builtin" }>,
  transport: Live2DAssetTransport,
  lifecycle: RuntimePetLoadOptions,
): PetRendererBootstrapOptions {
  return {
    root,
    diagnose: lifecycle.diagnose,
    onSurfaceChanged: lifecycle.onSurfaceChanged,
    assetUrl: (_petId, relativePath) => resolveBuiltinPetUrl(
      source.manifestUrl,
      relativePath,
      window.location.origin,
    ),
    loadLive2DAsset: (petId, manifest) => loadLive2DAsset(petId, manifest, transport),
  };
}

function manifestVersionOf(value: unknown): number {
  if (typeof value === "object" && value !== null && "schemaVersion" in value) {
    const version = (value as { schemaVersion?: unknown }).schemaVersion;
    return typeof version === "number" ? version : 0;
  }
  return 0;
}

function rejectCandidatePreviewFallback(
  runtime: PetRendererRuntime,
  manifest: unknown,
  options: RuntimePetLoadOptions,
): PetRendererRuntime {
  if (!options.allowPreviewFallback && manifestVersionOf(manifest) === 2 && runtime.kind() === "static-png") {
    runtime.host.destroy();
    throw new Error("preview fallback is not allowed for hot switching");
  }
  return runtime;
}

export async function loadRuntimePet(
  descriptor: RuntimePetDescriptor,
  root: HTMLElement,
  ports: RuntimePetLoaderPorts = defaultPorts,
  options: RuntimePetLoadOptions = {},
): Promise<MountedPetRuntime> {
  let manifest: unknown;
  let previewUrl: string;
  try {
    if (descriptor.source === "installed") {
      previewUrl = `pet-asset://localhost/${descriptor.petId}/assets/body.png`;
      manifest = await ports.readInstalledManifest(descriptor.petId);
      const runtime = rejectCandidatePreviewFallback(
        await ports.createRuntime(descriptor.petId, manifest, {
          root,
          diagnose: options.diagnose,
          onSurfaceChanged: options.onSurfaceChanged,
        }),
        manifest,
        options,
      );
      return {
        petId: descriptor.petId,
        ...runtime,
      };
    }

    const source = selectStartupPetSource(descriptor.petId);
    if (source.kind !== "builtin") throw new Error(`missing built-in pet source: ${descriptor.petId}`);
    previewUrl = source.previewUrl;
    const transport = ports.createBuiltinTransport(source.manifestUrl);
    manifest = await transport.readManifest(descriptor.petId);
    const runtime = rejectCandidatePreviewFallback(
      await ports.createRuntime(
        descriptor.petId,
        manifest,
        builtinRuntimeOptions(root, source, transport, options),
      ),
      manifest,
      options,
    );
    return {
      petId: descriptor.petId,
      ...runtime,
    };
  } catch (error) {
    options.diagnose?.({
      petId: descriptor.petId,
      manifestVersion: manifestVersionOf(manifest),
      stage: "manifest-load",
      message: error instanceof Error ? error.message : String(error),
    });
    if (!options.allowPreviewFallback) throw error;
    return {
      petId: descriptor.petId,
      ...(await ports.createPreviewRuntime(previewUrl!, {
        root,
        diagnose: options.diagnose,
        onSurfaceChanged: options.onSurfaceChanged,
      })),
    };
  }
}
