import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { CreationSnapshot } from "./contracts";

export type InvokePort = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export function createCreationApi(invoke: InvokePort) {
  return {
    start: (method: "upload" | "composer") =>
      invoke<CreationSnapshot>("creation_start", { method }),
    draft: () => invoke<CreationSnapshot | null>("creation_draft"),
    snapshot: (sessionId: string) =>
      invoke<CreationSnapshot>("creation_snapshot", { sessionId }),
    setName: (sessionId: string, displayName: string) =>
      invoke<CreationSnapshot>("creation_set_name", { sessionId, displayName }),
    abandon: (sessionId: string) =>
      invoke<void>("creation_abandon", { sessionId }),
  };
}

export const creationApi = createCreationApi(tauriInvoke);
