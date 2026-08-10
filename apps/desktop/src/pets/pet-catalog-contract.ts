import type { CreationMethod } from "../creation/contracts";

export type PetLifecycle =
  | "ready"
  | "generating"
  | "generationFailed"
  | "awaitingConfirm"
  | "compileRetryable"
  | "awaitingActivation"
  | "corrupt";

export interface PetCatalogEntry {
  petId: string;
  displayName: string;
  creationMethod: CreationMethod;
  sourceTemplateId: string | null;
  source: "builtin" | "user";
  species: "cat" | "dog";
  identityMode: string;
  createdAt: string | null;
  isCurrent: boolean;
  deletable: boolean;
  status: PetLifecycle;
  issue: string | null;
}

export interface CreationResume {
  petId: string;
  status: PetLifecycle;
  jobId: string | null;
  variantId: string | null;
  error: string | null;
}
