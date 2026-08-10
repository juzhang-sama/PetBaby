import { describe, expect, it, vi } from "vitest";
import { createCreationApi } from "./api";

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
});
