import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { MotionProfileV1 } from "../runtime/animated-image-manifest";
import type {
  AdoptionCatalogEntry,
  ComposerRecipe,
  CreationSnapshot,
  UploadCandidateAssets,
} from "./contracts";

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

export interface PhotoAvatarUpload {
  bytesB64: string;
  sha256: string;
}

export interface PhotoAvatarSnapshot {
  route?: "pixel-v1";
  sessionId: string;
  revision: number;
  step: string;
  providerJobId: string | null;
  profile: unknown;
  attempts: Record<string, number>;
  errorCode: string | null;
  errorMessage: string | null;
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
    composerCandidate: (sessionId: string, pngB64?: string) =>
      invoke<ComposerCandidateProjection>("creation_composer_candidate", { sessionId, pngB64 }),
    adoptionCatalog: () =>
      invoke<AdoptionCatalogEntry[]>("creation_adoption_catalog"),
    adoptionStart: (templateId: string, displayName: string) =>
      invoke<CreationSnapshot>("creation_adoption_start", { templateId, displayName }),
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
    uploadCandidate: (jobId: string) =>
      invoke<UploadCandidateAssets>("creation_upload_candidate_assets", { jobId }),
    recoverFinalization: () => invoke<RecoveryReport>("creation_recover_finalization"),
    photoAvatarConsent: (accept: boolean) =>
      invoke<boolean>("creation_photo_avatar_consent", { accept }),
    photoAvatarBegin: (
      sessionId: string,
      consentVersion: string,
      photos: PhotoAvatarUpload[],
    ) =>
      invoke<PhotoAvatarSnapshot>("creation_photo_avatar_begin", {
        sessionId,
        consentVersion,
        photos,
      }),
    photoAvatarStatus: (sessionId: string) =>
      invoke<PhotoAvatarSnapshot | null>("creation_photo_avatar_status", { sessionId }),
    photoAvatarRuntimeCheckPassed: (
      sessionId: string,
      revision: number,
      manifestSha256: string,
    ) =>
      invoke<PhotoAvatarSnapshot>("creation_photo_avatar_runtime_check_passed", {
        sessionId,
        revision,
        manifestSha256,
      }),
    photoAvatarCancel: (sessionId: string) =>
      invoke<PhotoAvatarSnapshot>("creation_photo_avatar_cancel", { sessionId }),
    photoAvatarRegenerate: (sessionId: string) =>
      invoke<PhotoAvatarSnapshot>("creation_photo_avatar_regenerate", { sessionId }),
    photoAvatarRevise: (sessionId: string, instruction: string) =>
      invoke<PhotoAvatarSnapshot>("creation_photo_avatar_revise", { sessionId, instruction }),
    photoAvatarPreviewManifest: (sessionId: string, revision: number) =>
      invoke<unknown>("creation_photo_avatar_preview_manifest", { sessionId, revision }),
    photoAvatarPreviewFileB64: (sessionId: string, revision: number, relativePath: string) =>
      invoke<string>("creation_photo_avatar_preview_file_b64", {
        sessionId,
        revision,
        relativePath,
      }),
  };
}

export const creationApi = createCreationApi(tauriInvoke);
