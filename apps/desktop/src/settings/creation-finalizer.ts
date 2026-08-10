import { invoke } from "@tauri-apps/api/core";
import type { CreationSnapshot } from "../creation/contracts";
import type { PetSwitchOptions, PetSwitchResult } from "../runtime/pet-switch-protocol";
import { requestPetSwitch } from "./pet-switch-client";

export interface PreparedCreation {
  requestId: string;
  sessionId: string;
  petId: string;
  variantId: string;
  alreadyCompleted: boolean;
}

export interface FinalizerPorts {
  prepare(sessionId: string, requestId: string): Promise<PreparedCreation>;
  switchPet(petId: string, options: PetSwitchOptions): Promise<PetSwitchResult>;
  abort(sessionId: string, error: string): Promise<CreationSnapshot>;
  cancel(requestId: string): Promise<void>;
}

const tauriFinalizerPorts: FinalizerPorts = {
  prepare: (sessionId, requestId) =>
    invoke<PreparedCreation>("creation_prepare_finalize", { sessionId, requestId }),
  switchPet: (petId, options) => requestPetSwitch(petId, options),
  abort: (sessionId, error) =>
    invoke<CreationSnapshot>("creation_abort_finalize", { sessionId, error }),
  cancel: (requestId) => invoke<void>("pet_cancel_switch", { requestId }),
};

export async function finalizeCreation(
  sessionId: string,
  ports: FinalizerPorts = tauriFinalizerPorts,
): Promise<PetSwitchResult> {
  const requestId = crypto.randomUUID();
  let prepared: PreparedCreation;
  try {
    prepared = await ports.prepare(sessionId, requestId);
  } catch (error) {
    await ports.cancel(requestId).catch(() => undefined);
    throw error;
  }
  if (prepared.alreadyCompleted) {
    return { ok: true, requestId, petId: prepared.petId };
  }

  let result: PetSwitchResult;
  try {
    result = await ports.switchPet(prepared.petId, {
      requestId,
      acceptedVariantId: prepared.variantId,
      creationSessionId: prepared.sessionId,
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    result = {
      ok: false,
      requestId,
      petId: prepared.petId,
      code: "pet-window-unavailable",
      message,
    };
  }
  if (!result.ok) {
    await ports.abort(prepared.sessionId, result.message).catch(() => undefined);
    await ports.cancel(requestId).catch(() => undefined);
  }
  return result;
}
