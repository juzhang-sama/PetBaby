import type { MountedPetRuntime } from "./pet-runtime-slot";
import type { RuntimePetDescriptor } from "./pet-switch-protocol";

export const BUILTIN_PET_ID = "pet-live2d-v1";

export interface StartupRuntimePorts {
  prepare(petId: string): Promise<RuntimePetDescriptor>;
  load(descriptor: RuntimePetDescriptor): Promise<MountedPetRuntime>;
  commit(petId: string): Promise<void>;
  onRecovery?(petId: string, error: unknown): void;
}

export interface StartupRuntimeResult {
  runtime: MountedPetRuntime;
  recoveredToBuiltin: boolean;
}

export async function loadStartupRuntime(
  activePetId: string,
  ports: StartupRuntimePorts,
): Promise<StartupRuntimeResult> {
  const descriptor = await ports.prepare(activePetId);
  try {
    return { runtime: await ports.load(descriptor), recoveredToBuiltin: false };
  } catch (error) {
    if (activePetId === BUILTIN_PET_ID) throw error;
    ports.onRecovery?.(activePetId, error);
  }

  const fallbackDescriptor = await ports.prepare(BUILTIN_PET_ID);
  const fallbackRuntime = await ports.load(fallbackDescriptor);
  try {
    await ports.commit(BUILTIN_PET_ID);
  } catch (error) {
    try {
      fallbackRuntime.host.destroy();
    } catch {
      // Preserve the persistence failure that prevented recovery.
    }
    throw error;
  }
  return { runtime: fallbackRuntime, recoveredToBuiltin: true };
}
