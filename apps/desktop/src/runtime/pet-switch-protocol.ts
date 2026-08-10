export const PET_SWITCH_REQUEST = "pet:switch-request";
export const PET_SWITCH_RESULT = "pet:switch-result";

export interface RuntimePetDescriptor {
  petId: string;
  source: "builtin" | "installed";
}

export interface PetSwitchRequest {
  requestId: string;
  petId: string;
  acceptedVariantId?: string;
  creationSessionId?: string;
}

export interface PetSwitchOptions {
  requestId?: string;
  acceptedVariantId?: string;
  creationSessionId?: string;
}

export type CommitReconciliationStatus = "notCommitted" | "compensated" | "unknown";

export interface CommitReconciliation {
  status: CommitReconciliationStatus;
  warning?: string;
}

export interface CommitCompensation {
  status: "compensated" | "unknown";
  warning?: string;
}

export type PetSwitchErrorCode =
  | "target-not-found"
  | "asset-corrupt"
  | "load-failed"
  | "blank-frame"
  | "persist-failed"
  | "request-stale"
  | "pet-window-unavailable";

export type PetSwitchResult =
  | { ok: true; requestId: string; petId: string; warning?: string }
  | {
    ok: false;
    requestId: string;
    petId: string;
    code: PetSwitchErrorCode;
    message: string;
  };
