import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { MotionProfileV1 } from "../runtime/animated-image-manifest";
import type { ComposerRecipe, CreationSnapshot } from "./contracts";

export type InvokePort = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export interface UploadJobRecord {
  jobId: string;
  petId: string;
  sessionId: string | null;
  prompt: string;
  refSha256: string;
  taskId: string | null;
  status: string;
  resultUrl: string | null;
  error: string | null;
  createdAt: string;
}

export interface UploadSource {
  dataUrl: string;
  refSha256: string;
}

export interface RecoveryReport {
  completedSessionIds: string[];
  retryableSessionIds: string[];
  cleanedSessionIds: string[];
  warnings: string[];
}

export interface ComposerCandidateProjection {
  snapshot: CreationSnapshot;
  bodyUrl: string;
  motionProfile: MotionProfileV1;
}

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
    composerSave: (sessionId: string, recipe: ComposerRecipe, currentStep: string) =>
      invoke<CreationSnapshot>("creation_composer_save", { sessionId, recipe, currentStep }),
    composerCandidate: (sessionId: string, pngB64: string) =>
      invoke<ComposerCandidateProjection>("creation_composer_candidate", { sessionId, pngB64 }),
    uploadStart: (
      sessionId: string,
      prompt: string,
      refPngB64: string,
      refSha256: string,
    ) =>
      invoke<string>("creation_upload_start", {
        sessionId,
        prompt,
        refPngB64,
        refSha256,
      }),
    uploadRetry: (sessionId: string, prompt: string) =>
      invoke<string>("creation_upload_retry", { sessionId, prompt }),
    uploadJobs: (sessionId: string) =>
      invoke<UploadJobRecord[]>("creation_upload_jobs", { sessionId }),
    uploadSource: (sessionId: string) =>
      invoke<UploadSource | null>("creation_upload_source", { sessionId }),
    recoverFinalization: () => invoke<RecoveryReport>("creation_recover_finalization"),
  };
}

export const creationApi = createCreationApi(tauriInvoke);
