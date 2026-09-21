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

/**
 * 两条产线的 route 取值。
 *
 * 与 Rust 侧 `PhotoAvatarRoute` / DB 里 `photo_avatar_runs.route` 的取值域**逐字一致**。
 *
 * ⚠️ **不要收窄这个联合**：`pixel-v1` 已于 2026-09-20 停用（不能再开新会话），
 * 但这个类型**双用** —— 除了当 `begin` 的入参，还要解析历史会话的 snapshot，
 * 那里 route 仍然是 `pixel-v1`。收窄会把老会话弄成读不出来。
 * 「不许把退役值往外送」由 `selectedRoute()`（前端输入面）与 Rust 的生成闸口负责。
 */
export type PhotoAvatarRoute = "pixel-v1" | "frame-video-v1";

export interface PhotoAvatarSnapshot {
  route?: PhotoAvatarRoute;
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
      route?: PhotoAvatarRoute,
    ) =>
      invoke<PhotoAvatarSnapshot>("creation_photo_avatar_begin", {
        sessionId,
        consentVersion,
        photos,
        // 不给 route = 走现役产线（Rust 侧同一个口径）。给了就必须是认得的取值，
        // 由 Rust 侧 `PhotoAvatarRoute::parse` 挡。
        ...(route === undefined ? {} : { route }),
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
