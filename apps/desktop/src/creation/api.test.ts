import { describe, expect, expectTypeOf, it, vi } from "vitest";
import { createCreationApi, type PhotoAvatarSnapshot } from "./api";

describe("creationApi", () => {
  it("invokes creation_start with the selected method", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-1" });
    const api = createCreationApi(invoke);

    await api.start("upload");

    expect(invoke).toHaveBeenCalledWith("creation_start", { method: "upload" });
  });

  it("invokes creation_draft without an args object", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    const api = createCreationApi(invoke);

    await api.draft();

    expect(invoke).toHaveBeenCalledWith("creation_draft");
  });

  it("invokes creation_snapshot with a camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-1" });
    const api = createCreationApi(invoke);

    await api.snapshot("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_snapshot", {
      sessionId: "session-1",
    });
  });

  it("invokes creation_set_name with camelCase arguments", async () => {
    const invoke = vi.fn().mockResolvedValue({ displayName: "团子" });
    const api = createCreationApi(invoke);

    await api.setName("session-1", "团子");

    expect(invoke).toHaveBeenCalledWith("creation_set_name", {
      sessionId: "session-1",
      displayName: "团子",
    });
  });

  it("invokes creation_abandon with a camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const api = createCreationApi(invoke);

    await api.abandon("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_abandon", {
      sessionId: "session-1",
    });
  });

  it("sends only the recipe mutation contract for composer saves", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-1" });
    const api = createCreationApi(invoke);
    const recipe = {
      recipeVersion: 1, packId: "cat-cute-v1", packVersion: 1, layerContractVersion: 1,
      bodyId: "body-round", earsId: "ears-round", eyesId: "eyes-amber",
      muzzleId: "muzzle-gentle", tailId: "tail-curl", colorId: "color-cream",
      patternId: "pattern-none",
    };

    await api.composerSave("session-1", recipe, "ears");

    expect(invoke).toHaveBeenCalledWith("creation_composer_save", {
      sessionId: "session-1", recipe, currentStep: "ears",
    });
  });

  it("cannot inject motion paths or ownership into composer candidate storage", async () => {
    const invoke = vi.fn().mockResolvedValue({ snapshot: { sessionId: "session-1" } });
    const api = createCreationApi(invoke);

    await api.composerCandidate("session-1", "encoded-png");

    expect(invoke).toHaveBeenCalledWith("creation_composer_candidate", {
      sessionId: "session-1",
      pngB64: "encoded-png",
    });
    expect(Object.keys(vi.mocked(invoke).mock.calls[0]![1]!)).toEqual(["sessionId", "pngB64"]);
  });

  it("starts upload generation with camelCase session ownership arguments", async () => {
    const invoke = vi.fn().mockResolvedValue("job-1");
    const api = createCreationApi(invoke);

    await api.uploadStart("session-1", "fluffy cat", "cG5n", "sha-1");

    expect(invoke).toHaveBeenCalledWith("creation_upload_start", {
      sessionId: "session-1",
      prompt: "fluffy cat",
      refPngB64: "cG5n",
      refSha256: "sha-1",
    });
  });

  it("retries upload generation using only the durable session and prompt", async () => {
    const invoke = vi.fn().mockResolvedValue("job-retry");
    const api = createCreationApi(invoke);

    await expect(api.uploadRetry("session-1", "prompt")).resolves.toBe("job-retry");

    expect(invoke).toHaveBeenCalledWith("creation_upload_retry", {
      sessionId: "session-1",
      prompt: "prompt",
    });
  });

  it("lists upload jobs by camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue([]);
    const api = createCreationApi(invoke);

    await api.uploadJobs("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_upload_jobs", {
      sessionId: "session-1",
    });
  });

  it("loads a durable upload source by camelCase sessionId", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    const api = createCreationApi(invoke);

    await api.uploadSource("session-1");

    expect(invoke).toHaveBeenCalledWith("creation_upload_source", {
      sessionId: "session-1",
    });
  });

  it("reconciles durable finalization without client-owned arguments", async () => {
    const invoke = vi.fn().mockResolvedValue({ completedSessionIds: [] });
    const api = createCreationApi(invoke);

    await api.recoverFinalization();

    expect(invoke).toHaveBeenCalledWith("creation_recover_finalization");
  });

  it("loads the backend-owned adoption catalog without client arguments", async () => {
    const invoke = vi.fn().mockResolvedValue([]);
    const api = createCreationApi(invoke);

    await api.adoptionCatalog();

    expect(invoke).toHaveBeenCalledWith("creation_adoption_catalog");
  });

  it("starts adoption with only template identity and a display name", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-adoption" });
    const api = createCreationApi(invoke);

    await api.adoptionStart("cat-misty", "雾雾");

    expect(invoke).toHaveBeenCalledWith("creation_adoption_start", {
      templateId: "cat-misty",
      displayName: "雾雾",
    });
    expect(Object.keys(vi.mocked(invoke).mock.calls[0]![1]!)).toEqual([
      "templateId",
      "displayName",
    ]);
  });

  it("submits all photos once and exposes no provider key", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-1" });
    const api = createCreationApi(invoke);

    await api.photoAvatarBegin("session-1", "photo-avatar-third-party-ai-lk888-no-delete-v2", [
      { bytesB64: "cG5nMQ==", sha256: "a".repeat(64) },
      { bytesB64: "cG5nMg==", sha256: "b".repeat(64) },
    ]);

    expect(invoke).toHaveBeenCalledWith(
      "creation_photo_avatar_begin",
      expect.objectContaining({
        sessionId: "session-1",
        consentVersion: "photo-avatar-third-party-ai-lk888-no-delete-v2",
        photos: expect.any(Array),
      }),
    );
    expect(JSON.stringify(invoke.mock.calls)).not.toContain("apiKey");
  });

  it("maps the photo avatar lifecycle commands without provider credentials", async () => {
    const invoke = vi.fn().mockResolvedValue({ sessionId: "session-1" });
    const api = createCreationApi(invoke);

    await api.photoAvatarConsent(true);
    await api.photoAvatarStatus("session-1");
    await api.photoAvatarRuntimeCheckPassed("session-1", 2, "c".repeat(64));
    await api.photoAvatarCancel("session-1");
    await api.photoAvatarRegenerate("session-1");
    await api.photoAvatarRevise("session-1", "fluffier tail");
    await api.photoAvatarPreviewManifest("session-1", 2);
    await api.photoAvatarPreviewFileB64("session-1", 2, "model3.json");

    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "creation_photo_avatar_consent",
      "creation_photo_avatar_status",
      "creation_photo_avatar_runtime_check_passed",
      "creation_photo_avatar_cancel",
      "creation_photo_avatar_regenerate",
      "creation_photo_avatar_revise",
      "creation_photo_avatar_preview_manifest",
      "creation_photo_avatar_preview_file_b64",
    ]);
    expect(JSON.stringify(invoke.mock.calls)).not.toContain("apiKey");
  });

  it("passes through a null photo avatar status for a pre-begin draft", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    const api = createCreationApi(invoke);

    const status = await api.photoAvatarStatus("durable-session");

    expectTypeOf(status).toEqualTypeOf<PhotoAvatarSnapshot | null>();
    expect(status).toBeNull();
    expect(invoke).toHaveBeenCalledWith("creation_photo_avatar_status", {
      sessionId: "durable-session",
    });
  });
});
