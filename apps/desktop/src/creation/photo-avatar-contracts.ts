import {
  BODY_MODULE_IDS_V1,
  type BodyModuleIdV1,
} from "../runtime-assets/cat-motion-spatial-profile";
export {
  parsePixelAppearanceProfileV1,
  parsePixelPhotoAvatarSnapshot,
  PIXEL_IDENTITY_TRAIT_KEYS,
  PIXEL_PHOTO_AVATAR_STEPS,
} from "./photo-avatar-pixel-contracts";
export type {
  PixelAppearanceProfileV1,
  PixelIdentityTraitKey,
  PixelIdentityTraitV1,
  PixelPhotoAvatarSnapshot,
  PixelPhotoAvatarStep,
  PixelStyleProfileId,
} from "./photo-avatar-pixel-contracts";

export const PHOTO_AVATAR_STEPS = [
  "collecting",
  "analyzeIdentity",
  "completeAppearance",
  "renderTextureAtlas",
  "buildV5",
  "runtimeCheckPending",
  "previewReady",
  "cleanupPending",
  "completed",
  "failed",
  "cancelled",
] as const;

export type PhotoAvatarStep = (typeof PHOTO_AVATAR_STEPS)[number];

export const PHOTO_AVATAR_ERROR_CODES = [
  "invalidInput",
  "auth",
  "quota",
  "contentPolicy",
  "unsupported",
  "network",
  "timeout",
  "provider5xx",
  "temporaryUnavailable",
  "localStorage",
] as const;

export type PhotoAvatarErrorCode = (typeof PHOTO_AVATAR_ERROR_CODES)[number];
export type TraitSource = "user" | "ai-completed";

export const IDENTITY_TRAIT_KEYS = [
  "faceShape",
  "faceProportions",
  "furColors",
  "markings",
  "eyeShape",
  "eyeColor",
  "earShape",
  "bodyType",
  "tail",
  "signatureMarks",
  "temperament",
] as const;

export type IdentityTraitKey = (typeof IDENTITY_TRAIT_KEYS)[number];

export interface IdentityTraitV1 {
  key: IdentityTraitKey;
  value: string;
  source: TraitSource;
  evidencePhotoIds: string[];
}

export interface AppearanceProfileV1 {
  schemaVersion: 1;
  species: "cat";
  style: "animated-film-soft-v1";
  bodyModuleId: BodyModuleIdV1;
  bodyModuleSource: TraitSource;
  traits: IdentityTraitV1[];
  completionSummary: string[];
}

export interface PhotoAvatarSnapshot {
  sessionId: string;
  revision: number;
  step: PhotoAvatarStep;
  providerJobId: string | null;
  profile: AppearanceProfileV1 | null;
  attempts: Partial<Record<"analyzeIdentity" | "completeAppearance" | "renderTextureAtlas", number>>;
  errorCode: PhotoAvatarErrorCode | null;
  errorMessage: string | null;
}

export interface PhotoAvatarRevisionRequest {
  instruction: string;
  lockedTraitKeys: IdentityTraitKey[];
}

const BODY_MODULE_IDS = new Set<string>(BODY_MODULE_IDS_V1);
const ERROR_CODES = new Set<string>(PHOTO_AVATAR_ERROR_CODES);
const IDENTITY_KEYS = new Set<string>(IDENTITY_TRAIT_KEYS);
const TRAIT_SOURCES = new Set<string>(["user", "ai-completed"] satisfies TraitSource[]);

export function parsePhotoAvatarErrorCode(input: unknown): PhotoAvatarErrorCode {
  if (typeof input !== "string" || !ERROR_CODES.has(input)) {
    fail("errorCode is not supported");
  }
  return input as PhotoAvatarErrorCode;
}

export function parseAppearanceProfileV1(input: unknown): AppearanceProfileV1 {
  const value = exactObject(input, "profile", [
    "schemaVersion",
    "species",
    "style",
    "bodyModuleId",
    "bodyModuleSource",
    "traits",
    "completionSummary",
  ]);
  if (value.schemaVersion !== 1) fail("schemaVersion must be 1");
  if (value.species !== "cat") fail("species must be cat");
  if (value.style !== "animated-film-soft-v1") fail("style must be animated-film-soft-v1");
  if (typeof value.bodyModuleId !== "string" || !BODY_MODULE_IDS.has(value.bodyModuleId)) {
    fail("bodyModuleId is not supported");
  }
  const bodyModuleSource = traitSource(value.bodyModuleSource, "bodyModuleSource");
  if (!Array.isArray(value.traits)) fail("traits must be an array");
  if (!Array.isArray(value.completionSummary)) fail("completionSummary must be an array");

  const completionSummary = value.completionSummary.map((entry, index) =>
    nonEmptyString(entry, `completionSummary[${index}]`)
  );
  const traits = value.traits.map((entry, index) => identityTrait(entry, index));
  const seen = new Set<IdentityTraitKey>();
  for (const trait of traits) {
    if (seen.has(trait.key)) fail(`duplicate trait key: ${trait.key}`);
    seen.add(trait.key);
    if (trait.source === "user" && trait.evidencePhotoIds.length === 0) {
      fail(`traits[${traits.indexOf(trait)}].evidencePhotoIds must contain at least one photo id`);
    }
    if (trait.source === "ai-completed" && !completionSummary.includes(trait.key)) {
      fail(`completionSummary must include ai-completed trait: ${trait.key}`);
    }
  }

  return {
    schemaVersion: 1,
    species: "cat",
    style: "animated-film-soft-v1",
    bodyModuleId: value.bodyModuleId as BodyModuleIdV1,
    bodyModuleSource,
    traits,
    completionSummary,
  };
}

export function parsePhotoAvatarRevisionRequest(input: unknown): PhotoAvatarRevisionRequest {
  const value = exactObject(input, "revisionRequest", ["instruction", "lockedTraitKeys"]);
  const instruction = nonEmptyString(value.instruction, "instruction");
  if (!Array.isArray(value.lockedTraitKeys)) fail("lockedTraitKeys must be an array");
  const lockedTraitKeys = value.lockedTraitKeys.map((entry, index) =>
    identityTraitKey(entry, `lockedTraitKeys[${index}]`)
  );
  return {
    instruction,
    lockedTraitKeys: [...new Set(lockedTraitKeys)].sort(),
  };
}

export function validateRevisionLock(
  before: AppearanceProfileV1,
  after: AppearanceProfileV1,
  lockedTraitKeys: readonly IdentityTraitKey[],
): void {
  const beforeByKey = new Map(before.traits.map((trait) => [trait.key, trait]));
  const afterByKey = new Map(after.traits.map((trait) => [trait.key, trait]));
  for (const key of lockedTraitKeys) {
    if (traitSignature(beforeByKey.get(key)) !== traitSignature(afterByKey.get(key))) {
      fail(`locked trait changed: ${key}`);
    }
  }
}

function identityTrait(input: unknown, index: number): IdentityTraitV1 {
  const path = `traits[${index}]`;
  const value = exactObject(input, path, ["key", "value", "source", "evidencePhotoIds"]);
  if (!Array.isArray(value.evidencePhotoIds)) fail(`${path}.evidencePhotoIds must be an array`);
  return {
    key: identityTraitKey(value.key, `${path}.key`),
    value: nonEmptyString(value.value, `${path}.value`),
    source: traitSource(value.source, `${path}.source`),
    evidencePhotoIds: value.evidencePhotoIds.map((entry, evidenceIndex) =>
      nonEmptyString(entry, `${path}.evidencePhotoIds[${evidenceIndex}]`)
    ),
  };
}

function identityTraitKey(input: unknown, path: string): IdentityTraitKey {
  if (typeof input !== "string" || !IDENTITY_KEYS.has(input)) fail(`${path} is not supported`);
  return input as IdentityTraitKey;
}

function traitSource(input: unknown, path: string): TraitSource {
  if (typeof input !== "string" || !TRAIT_SOURCES.has(input)) fail(`${path} is not supported`);
  return input as TraitSource;
}

function traitSignature(trait: IdentityTraitV1 | undefined): string {
  return JSON.stringify(trait === undefined ? null : {
    value: trait.value,
    source: trait.source,
    evidencePhotoIds: trait.evidencePhotoIds,
  });
}

function nonEmptyString(input: unknown, path: string): string {
  if (typeof input !== "string" || input.trim().length === 0) fail(`${path} must be a non-empty string`);
  return input.trim();
}

function exactObject(
  input: unknown,
  path: string,
  keys: readonly string[],
): Record<string, unknown> {
  if (typeof input !== "object" || input === null || Array.isArray(input)) fail(`${path} must be an object`);
  const value = input as Record<string, unknown>;
  const allowed = new Set(keys);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) fail(`${path} has unknown field: ${key}`);
  }
  for (const key of keys) {
    if (!Object.hasOwn(value, key)) fail(`${path}.${key} is required`);
  }
  return value;
}

function fail(message: string): never {
  throw new Error(message);
}
