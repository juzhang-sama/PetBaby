import { emitTo, listen } from "@tauri-apps/api/event";
import {
  PET_SWITCH_REQUEST,
  PET_SWITCH_RESULT,
  type PetSwitchRequest,
  type PetSwitchResult,
} from "../runtime/pet-switch-protocol";

export interface SwitchClientPorts {
  listen(handler: (result: PetSwitchResult) => void): Promise<() => void>;
  emit(request: PetSwitchRequest): Promise<void>;
}

const tauriSwitchPorts: SwitchClientPorts = {
  listen: async (handler) => listen<PetSwitchResult>(PET_SWITCH_RESULT, ({ payload }) => handler(payload)),
  emit: (request) => emitTo("pet", PET_SWITCH_REQUEST, request),
};

export async function requestPetSwitch(
  petId: string,
  acceptedVariantId?: string,
  ports: SwitchClientPorts = tauriSwitchPorts,
): Promise<PetSwitchResult> {
  const requestId = crypto.randomUUID();
  let resolveResult!: (result: PetSwitchResult) => void;
  const resultPromise = new Promise<PetSwitchResult>((resolve) => { resolveResult = resolve; });
  let timer: number;
  const unlisten = await ports.listen((result) => {
    if (result.requestId !== requestId) return;
    window.clearTimeout(timer);
    unlisten();
    resolveResult(result);
  });
  timer = window.setTimeout(() => {
    unlisten();
    resolveResult({
      ok: false,
      requestId,
      petId,
      code: "pet-window-unavailable",
      message: "桌面宠物窗口没有响应",
    });
  }, 10_000);

  const request: PetSwitchRequest = {
    requestId,
    petId,
    ...(acceptedVariantId ? { acceptedVariantId } : {}),
  };
  try {
    await ports.emit(request);
  } catch (error) {
    window.clearTimeout(timer);
    unlisten();
    return {
      ok: false,
      requestId,
      petId,
      code: "pet-window-unavailable",
      message: error instanceof Error ? error.message : String(error),
    };
  }
  return resultPromise;
}
