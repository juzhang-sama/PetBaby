import type { MountedPetRuntime } from "./pet-runtime-slot";
import type {
  CommitReconciliation,
  PetSwitchRequest,
  RuntimePetDescriptor,
} from "./pet-switch-protocol";

export const BUILTIN_PET_ID = "cat-a-standard-v1";

export interface StartupRuntimePorts {
  prepare(petId: string): Promise<RuntimePetDescriptor>;
  load(descriptor: RuntimePetDescriptor): Promise<MountedPetRuntime>;
  commit(petId: string): Promise<StartupFinalizationResult | void>;
  onRecovery?(petId: string, error: unknown): void;
}

export interface StartupRuntimeResult {
  runtime: MountedPetRuntime;
  recoveredToBuiltin: boolean;
  warning?: string;
}

export interface StartupFinalizationPorts {
  prepareSwitch(requestId: string, petId: string): Promise<void>;
  commit(request: PetSwitchRequest): Promise<void>;
  reconcileCommit(previousPetId: string, request: PetSwitchRequest): Promise<CommitReconciliation>;
  cancel(requestId: string): Promise<void>;
  finish(requestId: string): Promise<void>;
}

export interface StartupFinalizationResult {
  warning?: string;
}

export async function finalizeStartupRecovery(
  previousPetId: string,
  petId: string,
  ports: StartupFinalizationPorts,
  deadlineMs: number = 10_000,
): Promise<StartupFinalizationResult> {
  const request: PetSwitchRequest = { requestId: crypto.randomUUID(), petId };
  try {
    await withDeadline(ports.prepareSwitch(request.requestId, petId), deadlineMs, "startup prepare");
  } catch (error) {
    const warning = await boundedCleanup(ports.cancel(request.requestId), deadlineMs, "cancel");
    throw new Error(`${messageOf(error)}${warning}`);
  }

  try {
    await withDeadline(ports.commit(request), deadlineMs, "startup commit");
  } catch (commitError) {
    let reconciliation: CommitReconciliation;
    try {
      reconciliation = await withDeadline(
        ports.reconcileCommit(previousPetId, request),
        deadlineMs,
        "startup reconciliation",
      );
    } catch (error) {
      throw new Error(`${messageOf(commitError)}；启动恢复对账未知：${messageOf(error)}`);
    }
    if (reconciliation.status === "notCommitted") {
      const warning = await boundedCleanup(ports.cancel(request.requestId), deadlineMs, "cancel");
      throw new Error(`${messageOf(commitError)}${warning}`);
    }
    if (reconciliation.status === "compensated") {
      const warning = await boundedCleanup(ports.finish(request.requestId), deadlineMs, "finish");
      const detail = reconciliation.warning ? `；对账提示：${reconciliation.warning}` : "";
      throw new Error(`${messageOf(commitError)}${detail}${warning}`);
    }
    const detail = reconciliation.warning ? `：${reconciliation.warning}` : "";
    throw new Error(`${messageOf(commitError)}；启动恢复提交状态未知${detail}`);
  }

  const warning = await boundedCleanup(ports.finish(request.requestId), deadlineMs, "finish");
  return warning ? { warning: warning.slice(1) } : {};
}

async function boundedCleanup(
  operation: Promise<void>,
  deadlineMs: number,
  label: string,
): Promise<string> {
  try {
    await withDeadline(operation, deadlineMs, `startup ${label}`);
    return "";
  } catch (error) {
    return `；${label} 未确认：${messageOf(error)}`;
  }
}

function withDeadline<T>(operation: Promise<T>, deadlineMs: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = globalThis.setTimeout(() => reject(new Error(`${label} 超时`)), deadlineMs);
    operation.then(resolve, reject).finally(() => globalThis.clearTimeout(timer));
  });
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
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
  let finalization: StartupFinalizationResult | void;
  try {
    finalization = await ports.commit(BUILTIN_PET_ID);
  } catch (error) {
    try {
      fallbackRuntime.host.destroy();
    } catch {
      // Preserve the persistence failure that prevented recovery.
    }
    throw error;
  }
  return {
    runtime: fallbackRuntime,
    recoveredToBuiltin: true,
    ...(finalization?.warning ? { warning: finalization.warning } : {}),
  };
}
