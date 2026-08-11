export type CreationMethod = "upload" | "composer" | "adoption";

export type CreationSessionStatus =
  | "draft"
  | "candidateReady"
  | "finalizing"
  | "retryableFailure"
  | "completed"
  | "abandoned";

export interface ComposerRecipe {
  recipeVersion: number;
  packId: string;
  packVersion: number;
  layerContractVersion: number;
  bodyId: string;
  earsId: string;
  eyesId: string;
  muzzleId: string;
  tailId: string;
  colorId: string;
  patternId: string;
}

export interface CreationSnapshot {
  sessionId: string;
  petId: string;
  method: CreationMethod;
  status: CreationSessionStatus;
  lastStableStatus: CreationSessionStatus;
  currentStep: string;
  displayName: string | null;
  jobId: string | null;
  jobStatus: string | null;
  candidateId: string | null;
  recipe: ComposerRecipe | null;
  error: string | null;
}

export interface AdoptionTemplate {
  templateId: string;
  templateVersion: number;
  runtimeSchemaVersion: number;
  defaultName: string;
  personality: string;
  thumbnailPath: string;
  bodyPath: string;
  motionProfilePath: string;
  thumbnailSha256: string;
  bodySha256: string;
  motionProfileSha256: string;
}

export interface AdoptionCatalogEntry {
  template: AdoptionTemplate;
  adoptedPetId: string | null;
  retrySessionId: string | null;
}
