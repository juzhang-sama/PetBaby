import {
  parsePhotoAvatarErrorCode,
  type PhotoAvatarErrorCode,
  type TraitSource,
} from "./photo-avatar-contracts";

export const PIXEL_IDENTITY_TRAIT_KEYS = [
  "faceShape",
  "faceProportions",
  "eyeShape",
  "eyeColor",
  "earShape",
  "primaryFurColor",
  "secondaryFurColor",
  "faceMarkings",
  "chestMarkings",
  "pawMarkings",
  "bodyMarkings",
  "tailShape",
  "tailMarkings",
  "signatureMarks",
  "temperament",
] as const;

export type PixelIdentityTraitKey = (typeof PIXEL_IDENTITY_TRAIT_KEYS)[number];
export type PixelStyleProfileId = "pixel-style-v1" | "pixel-style-v2-animation-ready";

export const PIXEL_PHOTO_AVATAR_STEPS = [
  "collecting",
  "analyzeIdentity",
  "generatePixelAvatar",
  "qualityCheckPending",
  "runtimeCheckPending",
  "previewReady",
  "cleanupPending",
  "completed",
  "failed",
  "cancelled",
] as const;

export type PixelPhotoAvatarStep = (typeof PIXEL_PHOTO_AVATAR_STEPS)[number];

export type PixelIdentityTraitV1 = {
  readonly key: PixelIdentityTraitKey;
  readonly value: string;
  readonly source: TraitSource;
  readonly evidencePhotoIds: readonly string[];
};

export type PixelAppearanceProfileV1 = {
  readonly schemaVersion: 1;
  readonly species: "cat";
  readonly styleProfileId: PixelStyleProfileId;
  readonly traits: readonly PixelIdentityTraitV1[];
  readonly completionSummary: readonly PixelIdentityTraitKey[];
};

export type PixelPhotoAvatarSnapshot = {
  readonly route: "pixel-v1";
  readonly sessionId: string;
  readonly revision: number;
  readonly step: PixelPhotoAvatarStep;
  readonly providerJobId: string | null;
  readonly profile: PixelAppearanceProfileV1 | null;
  readonly attempts: Readonly<Partial<Record<"analyzeIdentity" | "generatePixelAvatar", number>>>;
  readonly errorCode: PhotoAvatarErrorCode | null;
  readonly errorMessage: string | null;
};

const PIXEL_IDENTITY_KEYS = new Set<string>(PIXEL_IDENTITY_TRAIT_KEYS);
const PIXEL_STEPS = new Set<string>(PIXEL_PHOTO_AVATAR_STEPS);

export function parsePixelAppearanceProfileV1(input: unknown): PixelAppearanceProfileV1 {
  const value = exactObject(input, "pixelProfile", [
    "schemaVersion", "species", "styleProfileId", "traits", "completionSummary",
  ]);
  if (value.schemaVersion !== 1) fail("schemaVersion must be 1");
  if (value.species !== "cat") fail("species must be cat");
  const styleProfileId = pixelStyleProfileId(value.styleProfileId);
  if (!Array.isArray(value.traits)) fail("traits must be an array");
  if (!Array.isArray(value.completionSummary)) fail("completionSummary must be an array");

  const completionSummary = value.completionSummary.map((entry, index) =>
    pixelIdentityTraitKey(entry, `completionSummary[${index}]`),
  );
  const traits = value.traits.map((entry, index) => pixelIdentityTrait(entry, index));
  const seen = new Set<PixelIdentityTraitKey>();
  for (const [index, trait] of traits.entries()) {
    if (seen.has(trait.key)) fail(`duplicate trait key: ${trait.key}`);
    seen.add(trait.key);
    if (trait.source === "user" && trait.evidencePhotoIds.length === 0) {
      fail(`traits[${index}].evidencePhotoIds must contain at least one photo id`);
    }
    if (trait.source === "ai-completed" && !completionSummary.includes(trait.key)) {
      fail(`completionSummary must include ai-completed trait: ${trait.key}`);
    }
  }

  return { schemaVersion: 1, species: "cat", styleProfileId, traits, completionSummary };
}

export function parsePixelPhotoAvatarSnapshot(input: unknown): PixelPhotoAvatarSnapshot {
  const value = exactObject(input, "pixelPhotoAvatarSnapshot", [
    "route", "sessionId", "revision", "step", "providerJobId", "profile",
    "attempts", "errorCode", "errorMessage",
  ]);
  if (value.route !== "pixel-v1") fail("route must be pixel-v1");
  const sessionId = nonEmptyString(value.sessionId, "sessionId");
  if (!Number.isInteger(value.revision) || Number(value.revision) < 0) fail("revision is invalid");
  const step = pixelPhotoAvatarStep(value.step);
  const providerJobId = value.providerJobId === null
    ? null
    : nonEmptyString(value.providerJobId, "providerJobId");
  const profile = value.profile === null ? null : parsePixelAppearanceProfileV1(value.profile);
  const attempts = parsePixelAttempts(value.attempts);
  const errorCode = value.errorCode === null ? null : parsePhotoAvatarErrorCode(value.errorCode);
  const errorMessage = value.errorMessage === null
    ? null
    : nonEmptyString(value.errorMessage, "errorMessage");
  return {
    route: "pixel-v1", sessionId, revision: Number(value.revision), step,
    providerJobId, profile, attempts, errorCode, errorMessage,
  };
}

function pixelIdentityTrait(input: unknown, index: number): PixelIdentityTraitV1 {
  const path = `traits[${index}]`;
  const value = exactObject(input, path, ["key", "value", "source", "evidencePhotoIds"]);
  if (!Array.isArray(value.evidencePhotoIds)) fail(`${path}.evidencePhotoIds must be an array`);
  return {
    key: pixelIdentityTraitKey(value.key, `${path}.key`),
    value: nonEmptyString(value.value, `${path}.value`),
    source: traitSource(value.source, `${path}.source`),
    evidencePhotoIds: value.evidencePhotoIds.map((entry, evidenceIndex) =>
      nonEmptyString(entry, `${path}.evidencePhotoIds[${evidenceIndex}]`),
    ),
  };
}

function pixelIdentityTraitKey(input: unknown, path: string): PixelIdentityTraitKey {
  if (typeof input !== "string" || !PIXEL_IDENTITY_KEYS.has(input)) fail(`${path} is not supported`);
  return input as PixelIdentityTraitKey;
}

function pixelStyleProfileId(input: unknown): PixelStyleProfileId {
  if (input === "pixel-style-v1" || input === "pixel-style-v2-animation-ready") return input;
  fail("styleProfileId is not supported");
}

function pixelPhotoAvatarStep(input: unknown): PixelPhotoAvatarStep {
  if (typeof input !== "string" || !PIXEL_STEPS.has(input)) fail("step is invalid");
  return input as PixelPhotoAvatarStep;
}

function parsePixelAttempts(input: unknown): PixelPhotoAvatarSnapshot["attempts"] {
  const value = exactObject(input, "attempts", ["analyzeIdentity", "generatePixelAvatar"]);
  const attempts: Partial<Record<"analyzeIdentity" | "generatePixelAvatar", number>> = {};
  for (const key of ["analyzeIdentity", "generatePixelAvatar"] as const) {
    if (Object.hasOwn(value, key)) {
      const attempt = value[key];
      if (!Number.isInteger(attempt) || Number(attempt) < 0) fail(`attempts.${key} is invalid`);
      attempts[key] = Number(attempt);
    }
  }
  return attempts;
}

function traitSource(input: unknown, path: string): TraitSource {
  if (input === "user" || input === "ai-completed") return input;
  fail(`${path} is not supported`);
}

function nonEmptyString(input: unknown, path: string): string {
  if (typeof input !== "string" || input.trim().length === 0) fail(`${path} must be a non-empty string`);
  return input.trim();
}

function exactObject(input: unknown, path: string, keys: readonly string[]): Record<string, unknown> {
  if (typeof input !== "object" || input === null || Array.isArray(input)) fail(`${path} must be an object`);
  const value = input as Record<string, unknown>;
  const allowed = new Set(keys);
  for (const key of Object.keys(value)) if (!allowed.has(key)) fail(`${path} has unknown field: ${key}`);
  for (const key of keys) if (!Object.hasOwn(value, key)) fail(`${path}.${key} is required`);
  return value;
}

function fail(message: string): never {
  throw new Error(message);
}
