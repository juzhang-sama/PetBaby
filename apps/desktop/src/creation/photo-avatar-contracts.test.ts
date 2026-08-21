import { describe, expect, it } from "vitest";
import {
  parseAppearanceProfileV1,
  parsePixelAppearanceProfileV1,
  parsePixelPhotoAvatarSnapshot,
  parsePhotoAvatarErrorCode,
  parsePhotoAvatarRevisionRequest,
  validateRevisionLock,
} from "./photo-avatar-contracts";

function validAppearanceProfile(): Record<string, unknown> {
  return {
    schemaVersion: 1,
    species: "cat",
    style: "animated-film-soft-v1",
    bodyModuleId: "body-balanced-v1",
    bodyModuleSource: "ai-completed",
    traits: [
      {
        key: "faceShape",
        value: "round",
        source: "user",
        evidencePhotoIds: ["photo-front"],
      },
      {
        key: "bodyType",
        value: "balanced",
        source: "ai-completed",
        evidencePhotoIds: [],
      },
      {
        key: "markings",
        value: "white chin",
        source: "user",
        evidencePhotoIds: ["photo-front", "photo-side"],
      },
    ],
    completionSummary: ["bodyType"],
  };
}

function validPixelProfile(styleProfileId: string): Record<string, unknown> {
  return {
    schemaVersion: 1,
    species: "cat",
    styleProfileId,
    traits: [{
      key: "eyeColor",
      value: "green",
      source: "user",
      evidencePhotoIds: ["photo-front"],
    }],
    completionSummary: [],
  };
}

describe("photo avatar contracts", () => {
  it("rejects an implicit balanced body and unknown provenance", () => {
    const input = validAppearanceProfile();
    delete input.bodyModuleSource;
    expect(() => parseAppearanceProfileV1(input)).toThrow("bodyModuleSource");
    expect(() => parseAppearanceProfileV1({
      ...validAppearanceProfile(),
      traits: [{ key: "furColors", value: "black", source: "template", evidencePhotoIds: [] }],
    })).toThrow("traits[0].source");
  });

  it("rejects invalid body modules, duplicate traits, and incomplete provenance evidence", () => {
    expect(() => parseAppearanceProfileV1({
      ...validAppearanceProfile(),
      bodyModuleId: "body-custom-v1",
    })).toThrow("bodyModuleId");
    expect(() => parseAppearanceProfileV1({
      ...validAppearanceProfile(),
      traits: [
        { key: "eyeColor", value: "green", source: "user", evidencePhotoIds: ["photo-front"] },
        { key: "eyeColor", value: "amber", source: "user", evidencePhotoIds: ["photo-side"] },
      ],
      completionSummary: [],
    })).toThrow("duplicate trait key: eyeColor");
    expect(() => parseAppearanceProfileV1({
      ...validAppearanceProfile(),
      traits: [{ key: "eyeColor", value: "green", source: "user", evidencePhotoIds: [] }],
      completionSummary: [],
    })).toThrow("traits[0].evidencePhotoIds");
    expect(() => parseAppearanceProfileV1({
      ...validAppearanceProfile(),
      traits: [{ key: "tail", value: "long", source: "ai-completed", evidencePhotoIds: [] }],
      completionSummary: [],
    })).toThrow("completionSummary");
  });

  it("rejects unknown profile fields", () => {
    expect(() => parseAppearanceProfileV1({
      ...validAppearanceProfile(),
      inferredBodyType: "balanced",
    })).toThrow("inferredBodyType");
  });

  it("rejects a local revision that changes a locked face trait", () => {
    const beforeProfile = parseAppearanceProfileV1(validAppearanceProfile());
    const afterProfile = parseAppearanceProfileV1({
      ...validAppearanceProfile(),
      traits: (validAppearanceProfile().traits as Record<string, unknown>[]).map((trait) =>
        trait.key === "faceShape" ? { ...trait, value: "triangular" } : trait
      ),
    });

    expect(() => validateRevisionLock(beforeProfile, afterProfile, ["faceShape", "markings"]))
      .toThrow("locked trait changed: faceShape");
  });

  it("normalizes revision instructions and sorted unique locked trait keys", () => {
    expect(parsePhotoAvatarRevisionRequest({
      instruction: "  make the tail fluffier  ",
      lockedTraitKeys: ["markings", "faceShape", "markings"],
    })).toEqual({
      instruction: "make the tail fluffier",
      lockedTraitKeys: ["faceShape", "markings"],
    });
    expect(() => parsePhotoAvatarRevisionRequest({
      instruction: " ",
      lockedTraitKeys: [],
    })).toThrow("instruction");
    expect(() => parsePhotoAvatarRevisionRequest({
      instruction: "change eyes",
      lockedTraitKeys: ["whiskerLength"],
    })).toThrow("lockedTraitKeys[0]");
    expect(() => parsePhotoAvatarRevisionRequest({
      instruction: "change eyes",
      lockedTraitKeys: [],
      sessionId: "session-1",
    })).toThrow("sessionId");
  });

  it("strictly parses the frozen error-code union", () => {
    expect(parsePhotoAvatarErrorCode("temporaryUnavailable")).toBe("temporaryUnavailable");
    expect(() => parsePhotoAvatarErrorCode("rateLimited")).toThrow("errorCode");
  });

  it.each(["pixel-style-v1", "pixel-style-v2-animation-ready"] as const)(
    "parses supported pixel profile %s",
    (styleProfileId) => {
      expect(parsePixelAppearanceProfileV1(validPixelProfile(styleProfileId)).styleProfileId)
        .toBe(styleProfileId);
    },
  );

  it("rejects unknown pixel style ids", () => {
    expect(() => parsePixelAppearanceProfileV1(validPixelProfile("pixel-style-v3")))
      .toThrow("styleProfileId");
  });

  it("preserves the supported style id in pixel snapshots", () => {
    const snapshot = parsePixelPhotoAvatarSnapshot({
      route: "pixel-v1",
      sessionId: "session-1",
      revision: 1,
      step: "previewReady",
      providerJobId: "job-1",
      profile: validPixelProfile("pixel-style-v2-animation-ready"),
      attempts: { analyzeIdentity: 1, generatePixelAvatar: 1 },
      errorCode: null,
      errorMessage: null,
    });

    expect(snapshot.profile?.styleProfileId).toBe("pixel-style-v2-animation-ready");
  });
});
