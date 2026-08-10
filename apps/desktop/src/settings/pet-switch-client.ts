import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import {
  PET_SWITCH_REQUEST,
  PET_SWITCH_RESULT,
  type PetSwitchOptions,
  type PetSwitchRequest,
  type PetSwitchResult,
} from "../runtime/pet-switch-protocol";

export interface SwitchClientPorts {
  listen(handler: (result: PetSwitchResult) => void): Promise<() => void>;
  emit(request: PetSwitchRequest): Promise<void>;
  cancel(requestId: string): Promise<void>;
}

const tauriSwitchPorts: SwitchClientPorts = {
  listen: async (handler) => listen<PetSwitchResult>(PET_SWITCH_RESULT, ({ payload }) => handler(payload)),
  emit: (request) => emitTo("pet", PET_SWITCH_REQUEST, request),
  cancel: (requestId) => invoke("pet_cancel_switch", { requestId }),
};

export async function requestPetSwitch(
  petId: string,
  options: PetSwitchOptions = {},
  ports: SwitchClientPorts = tauriSwitchPorts,
): Promise<PetSwitchResult> {
  const requestId = options.requestId?.trim() || crypto.randomUUID();
  let resolveResult!: (result: PetSwitchResult) => void;
  const resultPromise = new Promise<PetSwitchResult>((resolve) => { resolveResult = resolve; });
  let unlisten: (() => void) | undefined;
  let timer: number | undefined;
  let settled = false;
  const cleanup = (): void => {
    if (timer !== undefined) {
      try {
        window.clearTimeout(timer);
      } catch {
        // Cleanup must not prevent the protocol result from settling.
      }
      timer = undefined;
    }
    const dispose = unlisten;
    unlisten = undefined;
    try {
      dispose?.();
    } catch {
      // Cleanup must not prevent the protocol result from settling.
    }
  };
  const finish = (result: PetSwitchResult): void => {
    if (settled) return;
    settled = true;
    cleanup();
    resolveResult(result);
  };
  const unavailable = (error: unknown): PetSwitchResult => ({
    ok: false,
    requestId,
    petId,
    code: "pet-window-unavailable",
    message: error instanceof Error ? error.message : String(error),
  });
  const failUnavailable = async (error: unknown): Promise<void> => {
    if (settled) return;
    settled = true;
    cleanup();
    try {
      await ports.cancel(requestId);
    } catch {
      // The backend gate has a TTL fallback when explicit cancellation cannot be delivered.
    }
    resolveResult(unavailable(error));
  };

  const request: PetSwitchRequest = {
    requestId,
    petId,
    ...(options.acceptedVariantId !== undefined
      ? { acceptedVariantId: options.acceptedVariantId }
      : {}),
    ...(options.creationSessionId !== undefined
      ? { creationSessionId: options.creationSessionId }
      : {}),
  };
  try {
    unlisten = await ports.listen((result) => {
      if (result.requestId === requestId) finish(result);
    });
    if (settled) {
      cleanup();
      return resultPromise;
    }
    timer = window.setTimeout(() => {
      void failUnavailable(new Error("桌面宠物窗口没有响应"));
    }, 10_000);
    await ports.emit(request);
  } catch (error) {
    await failUnavailable(error);
  }
  return resultPromise;
}
